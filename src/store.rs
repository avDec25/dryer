//! Keeps the clips on disk so the line survives a restart.
//!
//! Each clip is one file in `%LOCALAPPDATA%\Dryer\history`, named by its content
//! hash, plus an index listing the hashes newest first. A copy therefore only
//! writes its own file, and files for clips that left the line are deleted.
//!
//! Clipboard contents can be sensitive, so every file is encrypted with Windows
//! DPAPI: only this user, on this machine, can read it. A clip that cannot be
//! encrypted is not saved at all (never written as plain text).

use std::ffi::c_void;
use std::fs;
use std::path::PathBuf;

use windows::Win32::Foundation::{HLOCAL, LocalFree};
use windows::Win32::Security::Cryptography::{CRYPT_INTEGER_BLOB, CryptProtectData, CryptUnprotectData};
use windows::Win32::System::DataExchange::RegisterClipboardFormatW;
use windows::core::PCWSTR;

use crate::card::MIN_CARD_H;
use crate::clipboard::{Clip, Kind, format_name, local_minutes, time_label};
use crate::physics::Motion;

const MAGIC: &[u8; 4] = b"DRY1";
/// Registered clipboard formats start here; their ids differ from boot to boot.
const FIRST_REGISTERED: u32 = 0xC000;

pub struct Store {
    dir: PathBuf,
}

impl Store {
    /// Open (creating if needed) the history folder. None if there is no place to keep it.
    pub fn open() -> Option<Self> {
        let base = std::env::var_os("LOCALAPPDATA")?;
        let dir = PathBuf::from(base).join("Dryer").join("history");
        fs::create_dir_all(&dir).ok()?;
        Some(Self { dir })
    }

    fn clip_path(&self, hash: u64) -> PathBuf {
        self.dir.join(format!("{hash:016x}.clip"))
    }

    fn index_path(&self) -> PathBuf {
        self.dir.join("index.txt")
    }

    /// Save the clips, newest first. Only clips not on disk yet are encrypted and
    /// written; files for clips no longer on the line are removed.
    pub fn save(&self, clips: &[&Clip]) {
        let mut kept: Vec<u64> = Vec::new();
        for clip in clips {
            let path = self.clip_path(clip.hash);
            if !path.exists() {
                if let Some(bytes) = serialize(clip).and_then(|plain| protect(&plain)) {
                    let tmp = path.with_extension("tmp");
                    if fs::write(&tmp, bytes).is_ok() {
                        let _ = fs::rename(&tmp, &path);
                    }
                }
            }
            if path.exists() {
                kept.push(clip.hash);
            }
        }

        let index: String = kept.iter().map(|h| format!("{h:016x}\n")).collect();
        let tmp = self.dir.join("index.tmp");
        if fs::write(&tmp, index).is_ok() {
            let _ = fs::rename(&tmp, self.index_path());
        }

        let keep_names: Vec<String> = kept.iter().map(|h| format!("{h:016x}")).collect();
        if let Ok(entries) = fs::read_dir(&self.dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let stale_ext = path.extension().is_some_and(|e| e == "clip" || e == "tmp");
                let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                if stale_ext && !keep_names.iter().any(|n| n == stem) {
                    let _ = fs::remove_file(path);
                }
            }
        }
    }

    /// Read the saved clips, newest first. Anything unreadable is skipped.
    pub fn load(&self) -> Vec<Clip> {
        let Ok(index) = fs::read_to_string(self.index_path()) else {
            return Vec::new();
        };
        index
            .lines()
            .filter_map(|line| u64::from_str_radix(line.trim(), 16).ok())
            .filter_map(|hash| {
                let bytes = fs::read(self.clip_path(hash)).ok()?;
                let clip = deserialize(&unprotect(&bytes)?)?;
                (clip.hash == hash).then_some(clip)
            })
            .collect()
    }

    /// Delete everything that was saved.
    pub fn clear(&self) {
        if let Ok(entries) = fs::read_dir(&self.dir) {
            for entry in entries.flatten() {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
}

// ---------------------------------------------------------------- encoding

fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put_bytes(out: &mut Vec<u8>, b: &[u8]) {
    put_u32(out, b.len() as u32);
    out.extend_from_slice(b);
}

fn kind_to_byte(kind: Kind) -> u8 {
    match kind {
        Kind::Text => 0,
        Kind::Image => 1,
        Kind::Files => 2,
        Kind::Other => 3,
    }
}

fn kind_from_byte(b: u8) -> Option<Kind> {
    Some(match b {
        0 => Kind::Text,
        1 => Kind::Image,
        2 => Kind::Files,
        3 => Kind::Other,
        _ => return None,
    })
}

/// The clip as bytes. Registered formats are stored by name: their ids change
/// between boots. A format whose name cannot be found is left out.
fn serialize(clip: &Clip) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.push(kind_to_byte(clip.kind));
    out.extend_from_slice(&clip.hash.to_le_bytes());
    put_u32(&mut out, clip.minutes);
    put_bytes(&mut out, clip.preview.as_bytes());

    match &clip.thumb_px {
        Some((w, h, px)) => {
            out.push(1);
            put_u32(&mut out, *w);
            put_u32(&mut out, *h);
            put_bytes(&mut out, px);
        }
        None => out.push(0),
    }

    let mut formats: Vec<(u32, String, &Vec<u8>)> = Vec::new();
    for (id, bytes) in &clip.formats {
        if *id >= FIRST_REGISTERED {
            if let Some(name) = format_name(*id) {
                formats.push((0, name, bytes));
            }
        } else {
            formats.push((*id, String::new(), bytes));
        }
    }
    if formats.is_empty() {
        return None;
    }
    put_u32(&mut out, formats.len() as u32);
    for (id, name, bytes) in formats {
        put_u32(&mut out, id);
        put_bytes(&mut out, name.as_bytes());
        put_bytes(&mut out, bytes);
    }
    Some(out)
}

/// A bounds-checked reader over a byte slice.
struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.at.checked_add(n)?;
        let s = self.b.get(self.at..end)?;
        self.at = end;
        Some(s)
    }
    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
    fn bytes(&mut self) -> Option<&'a [u8]> {
        let n = self.u32()? as usize;
        self.take(n)
    }
}

/// Rebuild a clip from `serialize`'s bytes. Its card is laid out afterwards.
fn deserialize(bytes: &[u8]) -> Option<Clip> {
    let mut r = Reader { b: bytes, at: 0 };
    if r.take(4)? != MAGIC {
        return None;
    }
    let kind = kind_from_byte(r.u8()?)?;
    let hash = r.u64()?;
    let minutes = r.u32()?;
    let preview = String::from_utf8(r.bytes()?.to_vec()).ok()?;

    let thumb_px = if r.u8()? == 1 {
        let (w, h) = (r.u32()?, r.u32()?);
        let px = r.bytes()?.to_vec();
        (px.len() == (w as usize) * (h as usize) * 4).then_some((w, h, px))
    } else {
        None
    };

    let count = r.u32()? as usize;
    let mut formats = Vec::new();
    for _ in 0..count.min(256) {
        let id = r.u32()?;
        let name = String::from_utf8(r.bytes()?.to_vec()).ok()?;
        let data = r.bytes()?.to_vec();
        let id = if name.is_empty() {
            id
        } else {
            // Register the format again: it may now have a different id.
            let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
            match unsafe { RegisterClipboardFormatW(PCWSTR(wide.as_ptr())) } {
                0 => continue,
                new_id => new_id,
            }
        };
        formats.push((id, data));
    }
    if formats.is_empty() {
        return None;
    }

    Some(Clip {
        kind,
        formats,
        hash,
        preview,
        thumb_px,
        thumb: None,
        sprite: None,
        layout: None,
        minutes,
        time_text: time_label(minutes, local_minutes()),
        time_layout: None,
        text_h: 0.0,
        card_h: MIN_CARD_H,
        motion: Motion::default(),
    })
}

// ---------------------------------------------------------------- DPAPI

/// Encrypt for the current user on this machine.
fn protect(plain: &[u8]) -> Option<Vec<u8>> {
    unsafe {
        let input = CRYPT_INTEGER_BLOB { cbData: plain.len() as u32, pbData: plain.as_ptr() as *mut u8 };
        let mut output = CRYPT_INTEGER_BLOB::default();
        CryptProtectData(&input, PCWSTR::null(), None, None, None, 0, &mut output).ok()?;
        take_blob(output)
    }
}

fn unprotect(cipher: &[u8]) -> Option<Vec<u8>> {
    unsafe {
        let input = CRYPT_INTEGER_BLOB { cbData: cipher.len() as u32, pbData: cipher.as_ptr() as *mut u8 };
        let mut output = CRYPT_INTEGER_BLOB::default();
        CryptUnprotectData(&input, None, None, None, None, 0, &mut output).ok()?;
        take_blob(output)
    }
}

/// Copy a system-allocated blob into a Vec and free it.
unsafe fn take_blob(blob: CRYPT_INTEGER_BLOB) -> Option<Vec<u8>> {
    unsafe {
        if blob.pbData.is_null() {
            return None;
        }
        let v = std::slice::from_raw_parts(blob.pbData, blob.cbData as usize).to_vec();
        let _ = LocalFree(Some(HLOCAL(blob.pbData as *mut c_void)));
        Some(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Clip {
        Clip {
            kind: Kind::Image,
            formats: vec![(13, b"h\0i\0".to_vec()), (8, vec![1, 2, 3, 4])],
            hash: 0xDEAD_BEEF_0123_4567,
            preview: "names\nhere".into(),
            thumb_px: Some((2, 1, vec![9, 8, 7, 255, 6, 5, 4, 255])),
            thumb: None,
            sprite: None,
            layout: None,
            minutes: 123_456,
            time_text: String::new(),
            time_layout: None,
            text_h: 0.0,
            card_h: MIN_CARD_H,
            motion: Motion::default(),
        }
    }

    #[test]
    fn clip_round_trips() {
        let c = sample();
        let back = deserialize(&serialize(&c).unwrap()).unwrap();
        assert_eq!(back.kind, Kind::Image);
        assert_eq!(back.hash, c.hash);
        assert_eq!(back.minutes, c.minutes);
        assert_eq!(back.preview, c.preview);
        assert_eq!(back.thumb_px, c.thumb_px);
        assert_eq!(back.formats, c.formats);
    }

    #[test]
    fn damaged_data_is_rejected() {
        let bytes = serialize(&sample()).unwrap();
        assert!(deserialize(&bytes[..bytes.len() - 3]).is_none());
        assert!(deserialize(b"nope").is_none());
        let mut wrong = bytes.clone();
        wrong[0] = b'X';
        assert!(deserialize(&wrong).is_none());
    }

    #[test]
    fn encryption_round_trips_and_hides_the_text() {
        let plain = b"a secret that must not appear on disk".to_vec();
        let cipher = protect(&plain).expect("DPAPI available");
        assert!(!cipher.windows(plain.len()).any(|w| w == plain.as_slice()));
        assert_eq!(unprotect(&cipher).unwrap(), plain);
    }
}
