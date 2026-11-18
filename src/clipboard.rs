//! Clipboard capture and restore (Win32).

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND};
use windows::Win32::Graphics::Direct2D::ID2D1Bitmap1;
use windows::Win32::Graphics::DirectWrite::IDWriteTextLayout;
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, EnumClipboardFormats, GetClipboardData,
    GetClipboardFormatNameW, GetClipboardSequenceNumber, IsClipboardFormatAvailable, OpenClipboard,
    RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock};
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::core::w;

use crate::card::MIN_CARD_H;
use crate::physics::Motion;
use crate::thumbs::{self, Thumb};

/// Largest total payload we will keep for one clip.
const MAX_BYTES: usize = 8 * 1024 * 1024;
/// Characters of text kept for display (the full text stays in `formats`).
const PREVIEW_CHARS: usize = 600;
const MAX_FILE_NAMES: usize = 8;
const MAX_FORMAT_NAMES: usize = 4;

const CF_DIB: u32 = 8;
const CF_UNICODETEXT: u32 = 13;
const CF_HDROP: u32 = 15;
const CF_DIBV5: u32 = 17;

/// Registered clipboard formats that are only plumbing, not worth naming on a card.
const NOISY_FORMATS: [&str; 8] = [
    "Ole Private Data",
    "DataObject",
    "Preferred DropEffect",
    "Shell IDList Array",
    "InShellDragLoop",
    "Clipboard Viewer Ignore",
    "FileName",
    "FileNameW",
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Text,
    Image,
    Files,
    /// Anything else: shapes, spreadsheet cells, app-specific data.
    Other,
}

pub struct Clip {
    pub kind: Kind,
    /// Raw clipboard formats, restored verbatim on click.
    pub formats: Vec<(u32, Vec<u8>)>,
    pub hash: u64,
    /// Text shown on the card: the start of a text clip, the names of copied
    /// files, or the kinds of data in an "other" clip.
    pub preview: String,
    /// Small BGRA thumbnail in system memory (for rebuilding GPU bitmaps).
    pub thumb_px: Option<Thumb>,
    /// GPU copy of the thumbnail, created lazily by the renderer.
    pub thumb: Option<ID2D1Bitmap1>,
    /// The whole card pre-rendered once, so frames only move and rotate it.
    pub sprite: Option<ID2D1Bitmap1>,
    /// DirectWrite layout of `preview`, built once at capture.
    pub layout: Option<IDWriteTextLayout>,
    /// When the clip was copied: local wall-clock minutes since 1970-01-01.
    pub minutes: u32,
    /// `minutes` as shown on the card, e.g. "Today, 22:52".
    pub time_text: String,
    pub time_layout: Option<IDWriteTextLayout>,
    /// Laid-out text height in DIPs.
    pub text_h: f32,
    /// Card height in DIPs, fitted to the content.
    pub card_h: f32,
    pub motion: Motion,
}

impl Clip {
    /// Text that carries more than its plain characters (HTML, RTF, ...), so a
    /// plain-text copy would differ from it. Only the formats Dryer keeps are
    /// stored, so anything besides the Unicode text is formatting.
    pub fn is_rich_text(&self) -> bool {
        self.kind == Kind::Text
            && self.formats.iter().any(|(f, _)| *f == CF_UNICODETEXT)
            && self.formats.iter().any(|(f, _)| *f != CF_UNICODETEXT)
    }
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

        // Encoded images that browsers and editors put next to (or instead of) a bitmap.
        let encoded = [
            format_id(w!("PNG")),
            format_id(w!("JFIF")),
            format_id(w!("GIF")),
        ];
        let html = format_id(w!("HTML Format"));
        let rtf = format_id(w!("Rich Text Format"));
        let wanted = [
            CF_UNICODETEXT, CF_DIB, CF_DIBV5, CF_HDROP, html, rtf, encoded[0], encoded[1], encoded[2],
        ];

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

        // Text, an image or files: show it as such. Anything else is kept whole, so
        // it can still be put back on the clipboard exactly as it was.
        let (kind, thumb_px, preview, formats) = match classify(&formats, encoded) {
            Some((kind, thumb, preview)) => (kind, thumb, preview, formats),
            None => {
                let (all, preview) = read_all_formats()?;
                (Kind::Other, None, preview, all)
            }
        };

        let hash = hash_formats(&formats);
        let minutes = local_minutes();

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
            time_text: time_label(minutes, minutes),
            time_layout: None,
            text_h: 0.0,
            card_h: MIN_CARD_H,
            motion: Motion::default(),
        })
    }
}

/// Decide what a clip is, and build its thumbnail and card text.
fn classify(formats: &[(u32, Vec<u8>)], encoded: [u32; 3]) -> Option<(Kind, Option<Thumb>, String)> {
    let get = |fmt: u32| formats.iter().find(|(f, _)| *f == fmt).map(|(_, b)| b);

    // A bitmap on the clipboard (the V5 form can carry transparency).
    let bitmap = get(CF_DIBV5)
        .and_then(|d| thumbs::from_dib(d))
        .or_else(|| get(CF_DIB).and_then(|d| thumbs::from_dib(d)));
    if bitmap.is_some() {
        return Some((Kind::Image, bitmap, String::new()));
    }
    // An encoded image (PNG, JPEG, GIF).
    for fmt in encoded {
        if let Some(t) = get(fmt).and_then(|d| thumbs::from_encoded(d)) {
            return Some((Kind::Image, Some(t), String::new()));
        }
    }
    if get(CF_DIB).is_some() || get(CF_DIBV5).is_some() {
        // A bitmap we cannot decode: still an image, just without a picture.
        return Some((Kind::Image, None, String::new()));
    }

    if let Some(drop) = get(CF_HDROP) {
        let paths = file_paths(drop);
        let names = paths
            .iter()
            .take(MAX_FILE_NAMES)
            .map(|p| p.rsplit(['\\', '/']).next().unwrap_or(p).to_string())
            .collect::<Vec<_>>()
            .join("\n");
        let thumb = paths.first().and_then(|p| thumbs::from_file(p));
        // A single image file reads as an image; anything else keeps its name.
        let single_image = paths.len() == 1 && thumbs::is_image_path(&paths[0]) && thumb.is_some();
        let kind = if single_image { Kind::Image } else { Kind::Files };
        return Some((kind, thumb, if single_image { String::new() } else { names }));
    }

    get(CF_UNICODETEXT).map(|text| (Kind::Text, None, text_preview(text)))
}

/// Every in-memory format on the clipboard, plus a short label naming the kinds of data.
unsafe fn read_all_formats() -> Option<(Vec<(u32, Vec<u8>)>, String)> {
    unsafe {
        let mut formats = Vec::new();
        let mut names: Vec<String> = Vec::new();
        let mut total = 0usize;
        let mut fmt = EnumClipboardFormats(0);
        while fmt != 0 {
            // Skip handle-based formats (bitmaps, palettes, metafiles, display copies).
            let by_handle = matches!(fmt, 2 | 3 | 9 | 14 | 0x80..=0x8F | 0x200..=0x2FF);
            if !by_handle {
                if let Some(bytes) = read_format(fmt) {
                    total += bytes.len();
                    if total > MAX_BYTES {
                        return None;
                    }
                    if fmt >= 0xC000 {
                        if let Some(name) = format_name(fmt) {
                            if !NOISY_FORMATS.contains(&name.as_str()) && !names.contains(&name) {
                                names.push(name);
                            }
                        }
                    }
                    formats.push((fmt, bytes));
                }
            }
            fmt = EnumClipboardFormats(fmt);
        }
        if formats.is_empty() {
            return None;
        }
        names.truncate(MAX_FORMAT_NAMES);
        let label = if names.is_empty() { "Clipboard data".to_string() } else { names.join("\n") };
        Some((formats, label))
    }
}

/// The name of a registered clipboard format (id 0xC000 and up).
pub fn format_name(fmt: u32) -> Option<String> {
    let mut buf = [0u16; 80];
    let n = unsafe { GetClipboardFormatNameW(fmt, &mut buf) };
    (n > 0).then(|| String::from_utf16_lossy(&buf[..n as usize]))
}

/// Content hash used to spot a clip that is already on the line. Registered
/// format ids change from one boot to the next, so those are hashed by name:
/// the same content must hash the same after a restart.
pub fn hash_formats(formats: &[(u32, Vec<u8>)]) -> u64 {
    let mut hasher = DefaultHasher::new();
    for (fmt, bytes) in formats {
        match (*fmt >= 0xC000).then(|| format_name(*fmt)).flatten() {
            Some(name) => name.hash(&mut hasher),
            None => fmt.hash(&mut hasher),
        }
        bytes.hash(&mut hasher);
    }
    hasher.finish()
}

/// Days since 1970-01-01 for a calendar date (proleptic Gregorian).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The calendar date (year, month 1..=12, day) for days since 1970-01-01.
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + (m <= 2) as i64, m, d)
}

/// The current local time as minutes since 1970-01-01 (a calendar position, not UTC).
pub fn local_minutes() -> u32 {
    let t = unsafe { GetLocalTime() };
    let days = days_from_civil(t.wYear as i64, t.wMonth as i64, t.wDay as i64);
    (days * 1440 + t.wHour as i64 * 60 + t.wMinute as i64) as u32
}

/// How a capture time reads on a card: "Today, 22:52", "Yesterday, 09:05" or "3 Nov, 14:20".
pub fn time_label(minutes: u32, now: u32) -> String {
    const MONTHS: [&str; 12] =
        ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let (day, minute_of_day) = (minutes / 1440, minutes % 1440);
    let clock = format!("{:02}:{:02}", minute_of_day / 60, minute_of_day % 60);
    match (now / 1440).saturating_sub(day) {
        0 => format!("Today, {clock}"),
        1 => format!("Yesterday, {clock}"),
        _ => {
            let (_, m, d) = civil_from_days(day as i64);
            format!("{d} {}, {clock}", MONTHS[(m - 1) as usize])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calendar_conversion_round_trips() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
        for days in [0, 59, 60, 365, 11_016, 11_017, 20_000, 25_000] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days, "day {days}");
        }
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
    }

    #[test]
    fn time_labels() {
        let day = |d: i64, h: u32, m: u32| (d * 1440) as u32 + h * 60 + m;
        let nov17 = days_from_civil(2026, 11, 17);
        let now = day(nov17, 23, 15);
        assert_eq!(time_label(day(nov17, 9, 5), now), "Today, 09:05");
        assert_eq!(time_label(day(nov17 - 1, 22, 40), now), "Yesterday, 22:40");
        assert_eq!(time_label(day(nov17 - 14, 14, 20), now), "3 Nov, 14:20");
    }
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

/// Full paths from a CF_HDROP (DROPFILES) payload.
fn file_paths(b: &[u8]) -> Vec<String> {
    if b.len() < 20 {
        return Vec::new();
    }
    let off = (u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize).min(b.len());
    let wide = u32::from_le_bytes([b[16], b[17], b[18], b[19]]) != 0;
    let body = &b[off..];
    if wide {
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
    }
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
