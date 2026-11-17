//! Card geometry, layout along the line, and DirectWrite text layout.
//! Width is fixed; height follows the content, clamped to [MIN_CARD_H, MAX_CARD_H].

use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
    DWRITE_FONT_WEIGHT_NORMAL, DWRITE_TEXT_METRICS, DWRITE_TRIMMING,
    DWRITE_TRIMMING_GRANULARITY_CHARACTER, DWRITE_WORD_WRAPPING_NO_WRAP,
    DWRITE_WORD_WRAPPING_WRAP, DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat,
    IDWriteTextLayout,
};
use windows::core::{Result, w};

use crate::clipboard::{Clip, Kind};

pub const CARD_W: f32 = 150.0;
/// Gap between neighbouring cards on the line.
pub const CARD_GAP: f32 = 26.0;
pub const MARGIN: f32 = 24.0;
pub const MIN_CARD_H: f32 = 100.0;
pub const MAX_CARD_H: f32 = 220.0;
/// Width of the glass frame around the content panel.
pub const INSET: f32 = 5.0;
/// Padding inside the content panel.
pub const TEXT_PAD: f32 = 10.0;
/// Yellow notes strip at the top of a text card.
pub const STRIP_H: f32 = 6.0;
/// Space for the timestamp line.
pub const TIME_H: f32 = 14.0;
/// Icon above a file card's names, and the taller picture when there is a thumbnail.
pub const FILE_GLYPH_H: f32 = 44.0;
pub const FILE_THUMB_H: f32 = 80.0;
pub const TEXT_W: f32 = CARD_W - 2.0 * (INSET + TEXT_PAD);
const IMG_MIN_H: f32 = 60.0;
const IMG_MAX_H: f32 = MAX_CARD_H - 2.0 * INSET;

/// Top of the body text inside a card.
pub fn text_top(clip: &Clip) -> f32 {
    match clip.kind {
        Kind::Files | Kind::Other => {
            let media = if clip.thumb_px.is_some() { FILE_THUMB_H } else { FILE_GLYPH_H };
            INSET + TEXT_PAD + media
        }
        _ => INSET + STRIP_H + TEXT_PAD,
    }
}

/// Space below the body text: timestamp line plus padding.
pub const FOOTER_H: f32 = TEXT_PAD + TIME_H + TEXT_PAD + INSET;

/// Distance between neighbouring cards for `n` cards across `width`.
pub fn spacing(n: usize, width: f32) -> f32 {
    let fit = (width - 2.0 * MARGIN - CARD_W) / (n.max(2) - 1) as f32;
    (CARD_W + CARD_GAP).min(fit)
}

/// Rest x (centre) of slot `i` with `n` cards on the line. The group is centred;
/// slot 0, the newest, is rightmost, so older cards move left as more arrive.
pub fn slot_x(i: usize, n: usize, width: f32) -> f32 {
    let sp = spacing(n, width);
    let mid = (n.max(1) - 1) as f32 / 2.0;
    width / 2.0 + (mid - i as f32) * sp
}

pub struct Engine {
    factory: IDWriteFactory,
    body: IDWriteTextFormat,
    small: IDWriteTextFormat,
}

impl Engine {
    pub fn new() -> Result<Self> {
        unsafe {
            let factory: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let body = factory.CreateTextFormat(
                w!("Segoe UI"),
                None,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                12.0,
                w!("en-us"),
            )?;
            body.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP)?;
            // Overflowing text ends with an ellipsis on the last visible line.
            let sign = factory.CreateEllipsisTrimmingSign(&body)?;
            body.SetTrimming(
                &DWRITE_TRIMMING {
                    granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
                    delimiter: 0,
                    delimiterCount: 0,
                },
                &sign,
            )?;

            let small = factory.CreateTextFormat(
                w!("Segoe UI"),
                None,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                10.0,
                w!("en-us"),
            )?;
            small.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
            Ok(Self { factory, body, small })
        }
    }

    fn layout(&self, text: &str, format: &IDWriteTextFormat, w: f32, h: f32) -> Result<IDWriteTextLayout> {
        let wide: Vec<u16> = text.encode_utf16().collect();
        unsafe { self.factory.CreateTextLayout(&wide, format, w, h) }
    }

    /// Lay out the clip's text and fix its card height. Runs once per clip at capture.
    pub fn prepare(&self, clip: &mut Clip) -> Result<()> {
        clip.time_layout = Some(self.layout(&clip.time_text, &self.small, TEXT_W, TIME_H)?);
        match clip.kind {
            Kind::Image => {
                clip.card_h = match clip.thumb_px.as_ref() {
                    Some((tw, th, _)) => {
                        let inner_w = CARD_W - 2.0 * INSET;
                        let img_h = (*th as f32 * inner_w / *tw as f32).clamp(IMG_MIN_H, IMG_MAX_H);
                        (img_h + 2.0 * INSET).clamp(MIN_CARD_H, MAX_CARD_H)
                    }
                    None => MIN_CARD_H,
                };
            }
            Kind::Text | Kind::Files | Kind::Other => {
                let top = text_top(clip);
                let max_h = MAX_CARD_H - top - FOOTER_H;
                clip.layout = None;
                clip.text_h = 0.0;
                if !clip.preview.is_empty() {
                    let layout = self.layout(&clip.preview, &self.body, TEXT_W, max_h)?;
                    let mut metrics = DWRITE_TEXT_METRICS::default();
                    unsafe { layout.GetMetrics(&mut metrics)? };
                    clip.text_h = metrics.height.min(max_h);
                    clip.layout = Some(layout);
                }
                clip.card_h = (top + clip.text_h + FOOTER_H).clamp(MIN_CARD_H, MAX_CARD_H);
            }
        }
        Ok(())
    }
}
