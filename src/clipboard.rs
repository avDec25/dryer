//! Clipboard capture and restore (Win32), plus DIB thumbnail decoding.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND};
use windows::Win32::Graphics::Direct2D::ID2D1Bitmap1;
use windows::Win32::Graphics::DirectWrite::IDWriteTextLayout;
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, GetClipboardSequenceNumber,
    IsClipboardFormatAvailable, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock};
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::core::w;

use crate::card::MIN_CARD_H;
use crate::physics::Motion;

/// Largest total payload we will keep for one clip.
const MAX_BYTES: usize = 8 * 1024 * 1024;
/// Thumbnail box in pixels, aspect preserved.
const THUMB_MAX_W: u32 = 260;
const THUMB_MAX_H: u32 = 200;
/// Characters of text kept for display (the full text stays in `formats`).
const PREVIEW_CHARS: usize = 600;
const MAX_FILE_NAMES: usize = 8;

const CF_DIB: u32 = 8;
const CF_UNICODETEXT: u32 = 13;
const CF_HDROP: u32 = 15;
const CF_DIBV5: u32 = 17;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Text,
    Image,
    Files,
}

pub struct Clip {
    pub kind: Kind,
    /// Raw clipboard formats, restored verbatim on click.
    pub formats: Vec<(u32, Vec<u8>)>,
    pub hash: u64,
    /// Text shown on the card: the start of a text clip, or the names of copied files.
    pub preview: String,
    /// Small BGRA thumbnail in system memory (for rebuilding GPU bitmaps).
    pub thumb_px: Option<(u32, u32, Vec<u8>)>,
    /// GPU copy of the thumbnail, created lazily by the renderer.
    pub thumb: Option<ID2D1Bitmap1>,
    /// The whole card pre-rendered once, so frames only move and rotate it.
    pub sprite: Option<ID2D1Bitmap1>,
    /// DirectWrite layout of `preview`, built once at capture.
    pub layout: Option<IDWriteTextLayout>,
    /// When the clip was copied, e.g. "Today, 22:52".
    pub time_text: String,
    pub time_layout: Option<IDWriteTextLayout>,
    /// Laid-out text height in DIPs.
    pub text_h: f32,
    /// Card height in DIPs, fitted to the content.
    pub card_h: f32,
    pub motion: Motion,
}

pub enum Capture {
    /// Another app holds the clipboard; try again shortly.
    Busy,
    /// Nothing to keep (empty, private, or too large).
    Skip,
    Got(Clip),
}

fn format_id(name: windows::core::PCWSTR) -> u32 {
    unsafe { RegisterClipboardFormatW(name) }
}

fn is_available(fmt: u32) -> bool {
    unsafe { IsClipboardFormatAvailable(fmt).is_ok() }
}

/// Copy the bytes behind one clipboard format. Returns None for delayed or empty formats.
unsafe fn read_format(fmt: u32) -> Option<Vec<u8>> {
    unsafe {
        let handle = GetClipboardData(fmt).ok()?;
        if handle.is_invalid() {
            return None;
        }
        let hg = HGLOBAL(handle.0);
        let size = GlobalSize(hg);
        if size == 0 {
            return None;
        }
        let p = GlobalLock(hg) as *const u8;
        if p.is_null() {
            return None;
        }
        let bytes = std::slice::from_raw_parts(p, size).to_vec();
        let _ = GlobalUnlock(hg);
        Some(bytes)
    }
}

/// Read the current clipboard into a clip, honouring privacy flags from password managers.
pub fn capture(hwnd: HWND) -> Capture {
    unsafe {
        if OpenClipboard(Some(hwnd)).is_err() {
            return Capture::Busy;
        }
        let result = read_clip();
        let _ = CloseClipboard();
        match result {
            Some(clip) => Capture::Got(clip),
            None => Capture::Skip,
        }
    }
}

unsafe fn read_clip() -> Option<Clip> {
    unsafe {
        let exclude = format_id(w!("ExcludeClipboardContentFromMonitorProcessing"));
        if is_available(exclude) {
            return None;
        }
        let include = format_id(w!("CanIncludeInClipboardHistory"));
        if is_available(include)
            && read_format(include).is_some_and(|b| b.len() >= 4 && b[..4] == [0, 0, 0, 0])
        {
            return None;
        }

        let html = format_id(w!("HTML Format"));
        let rtf = format_id(w!("Rich Text Format"));
        let wanted = [CF_UNICODETEXT, CF_DIB, CF_DIBV5, CF_HDROP, html, rtf];

        let mut formats: Vec<(u32, Vec<u8>)> = Vec::new();
        let mut total = 0usize;
        for fmt in wanted {
            if !is_available(fmt) {
                continue;
            }
            if let Some(bytes) = read_format(fmt) {
                total += bytes.len();
                if total > MAX_BYTES {
                    return None;
                }
                formats.push((fmt, bytes));
            }
        }
        if formats.is_empty() {
            return None;
        }

        let get = |fmt: u32| formats.iter().find(|(f, _)| *f == fmt).map(|(_, b)| b);
        let (kind, thumb_px, preview) = if let Some(dib) = get(CF_DIB).or(get(CF_DIBV5)) {
            (Kind::Image, thumb_from_dib(dib), String::new())
        } else if let Some(drop) = get(CF_HDROP) {
            (Kind::Files, None, file_names(drop))
        } else if let Some(text) = get(CF_UNICODETEXT) {
            (Kind::Text, None, text_preview(text))
        } else {
            return None;
        };

        let mut hasher = DefaultHasher::new();
        for (fmt, bytes) in &formats {
            fmt.hash(&mut hasher);
            bytes.hash(&mut hasher);
        }

        Some(Clip {
            kind,
            formats,
            hash: hasher.finish(),
            preview,
            thumb_px,
            thumb: None,
            sprite: None,
            layout: None,
            time_text: now_text(),
            time_layout: None,
            text_h: 0.0,
            card_h: MIN_CARD_H,
            motion: Motion::default(),
        })
    }
}

/// Local time of capture, shown at the bottom of the card.
fn now_text() -> String {
    let t = unsafe { GetLocalTime() };
    format!("Today, {:02}:{:02}", t.wHour, t.wMinute)
}

/// The first part of a UTF-16 text payload, with line endings normalised.
fn text_preview(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .take_while(|&u| u != 0)
        .take(PREVIEW_CHARS * 2)
        .collect();
    let text = String::from_utf16_lossy(&units);
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\t', "    ")
        .chars()
        .take(PREVIEW_CHARS)
        .collect::<String>()
        .trim_end()
        .to_string()
}

/// File names from a CF_HDROP (DROPFILES) payload, one per line.
fn file_names(b: &[u8]) -> String {
    if b.len() < 20 {
        return String::new();
    }
    let off = (u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize).min(b.len());
    let wide = u32::from_le_bytes([b[16], b[17], b[18], b[19]]) != 0;
    let body = &b[off..];
    let paths: Vec<String> = if wide {
        let units: Vec<u16> = body
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        units
            .split(|&u| u == 0)
            .filter(|s| !s.is_empty())
            .map(String::from_utf16_lossy)
            .collect()
    } else {
        body.split(|&c| c == 0)
            .filter(|s| !s.is_empty())
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .collect()
    };
    paths
        .iter()
        .take(MAX_FILE_NAMES)
        .map(|p| p.rsplit(['\\', '/']).next().unwrap_or(p).to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Decode a 24/32 bpp uncompressed DIB into a small top-down BGRA thumbnail.
fn thumb_from_dib(b: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    if b.len() < 40 {
        return None;
    }
    let bi_size = u32::from_le_bytes(b[0..4].try_into().ok()?) as usize;
    let w = i32::from_le_bytes(b[4..8].try_into().ok()?);
    let h_raw = i32::from_le_bytes(b[8..12].try_into().ok()?);
    let bpp = u16::from_le_bytes(b[14..16].try_into().ok()?) as usize;
    let comp = u32::from_le_bytes(b[16..20].try_into().ok()?);

    if bi_size < 40 || w <= 0 || h_raw == 0 || !(bpp == 24 || bpp == 32) || !(comp == 0 || comp == 3) {
        return None;
    }
    let w = w as usize;
    let h = h_raw.unsigned_abs() as usize;
    let bottom_up = h_raw > 0;

    // BI_BITFIELDS with a 40-byte header stores three 4-byte masks after it.
    let mut offset = bi_size;
    if comp == 3 && bi_size == 40 {
        offset += 12;
    }
    let bytes_pp = bpp / 8;
    let stride = (w * bpp).div_ceil(32) * 4;
    if b.len() < offset + stride * h {
        return None;
    }

    let scale = (THUMB_MAX_W as f64 / w as f64)
        .min(THUMB_MAX_H as f64 / h as f64)
        .min(1.0);
    let tw = ((w as f64 * scale).round() as usize).max(1);
    let th = ((h as f64 * scale).round() as usize).max(1);

    let mut px = vec![255u8; tw * th * 4];
    for ty in 0..th {
        let sy = ty * h / th;
        let row = if bottom_up { h - 1 - sy } else { sy };
        let base = offset + row * stride;
        for tx in 0..tw {
            let sx = tx * w / tw;
            let s = base + sx * bytes_pp;
            let d = (ty * tw + tx) * 4;
            px[d] = b[s];
            px[d + 1] = b[s + 1];
            px[d + 2] = b[s + 2];
        }
    }
    Some((tw as u32, th as u32, px))
}

/// Put the clip's formats on the clipboard. Returns the new sequence number so the
/// listener can ignore the resulting change notification.
pub fn write(hwnd: HWND, formats: &[(u32, Vec<u8>)]) -> Option<u32> {
    unsafe {
        OpenClipboard(Some(hwnd)).ok()?;
        let _ = EmptyClipboard();
        for (fmt, bytes) in formats {
            let Ok(hg) = GlobalAlloc(GMEM_MOVEABLE, bytes.len()) else {
                continue;
            };
            let p = GlobalLock(hg) as *mut u8;
            if p.is_null() {
                let _ = GlobalFree(Some(hg));
                continue;
            }
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
            let _ = GlobalUnlock(hg);
            // On success the system owns the memory; on failure we must free it.
            if SetClipboardData(*fmt, Some(HANDLE(hg.0))).is_err() {
                let _ = GlobalFree(Some(hg));
            }
        }
        let _ = CloseClipboard();
        Some(GetClipboardSequenceNumber())
    }
}
