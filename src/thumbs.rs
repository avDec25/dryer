//! Small BGRA (premultiplied) thumbnails for clipboard images and files.
//!
//! - Bitmaps on the clipboard (DIBs) are decoded and shrunk here.
//! - Encoded images (PNG, JPEG, GIF) go through the Windows Imaging Component.
//! - Copied files get the same thumbnail or icon Explorer shows for them.

use std::ffi::c_void;

use windows::Win32::Foundation::SIZE;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteObject, GetDC, GetDIBits,
    GetObjectW, HBITMAP, HGDIOBJ, ReleaseDC,
};
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICImagingFactory, IWICPalette,
    WICBitmapDitherTypeNone, WICBitmapInterpolationModeFant, WICBitmapPaletteTypeCustom,
    WICDecodeMetadataCacheOnDemand,
};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IBindCtx, IStream};
use windows::Win32::UI::Shell::{
    IShellItem, IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_ICONONLY,
    SIIGBF_INCACHEONLY, SIIGBF_THUMBNAILONLY,
};
use windows::core::{IUnknown, Interface, PCWSTR};

/// Width, height and premultiplied BGRA pixels, top-down.
pub type Thumb = (u32, u32, Vec<u8>);

/// Thumbnail box in pixels, aspect preserved.
pub const THUMB_MAX_W: u32 = 260;
pub const THUMB_MAX_H: u32 = 200;

const IMAGE_EXTENSIONS: [&str; 14] = [
    "png", "jpg", "jpeg", "jpe", "jfif", "gif", "bmp", "webp", "tif", "tiff", "ico", "heic", "heif",
    "avif",
];

pub fn is_image_path(path: &str) -> bool {
    path.rsplit('.')
        .next()
        .map(|e| e.to_ascii_lowercase())
        .is_some_and(|e| IMAGE_EXTENSIONS.contains(&e.as_str()))
}

/// Size that fits (w, h) inside the thumbnail box without enlarging.
fn fit(w: usize, h: usize) -> (usize, usize) {
    let scale = (THUMB_MAX_W as f64 / w as f64)
        .min(THUMB_MAX_H as f64 / h as f64)
        .min(1.0);
    (
        ((w as f64 * scale).round() as usize).max(1),
        ((h as f64 * scale).round() as usize).max(1),
    )
}

/// Value of the bits selected by `mask`, scaled to 0..=255.
fn channel(v: u32, mask: u32) -> u8 {
    if mask == 0 {
        return 0;
    }
    let shift = mask.trailing_zeros();
    let max = (mask >> shift) as u64;
    ((((v & mask) >> shift) as u64 * 255) / max) as u8
}

/// Decode an uncompressed DIB (1, 4, 8, 16, 24 or 32 bits per pixel) into a thumbnail.
pub fn from_dib(b: &[u8]) -> Option<Thumb> {
    if b.len() < 40 {
        return None;
    }
    let u32_at = |o: usize| -> Option<u32> { Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?)) };
    let bi_size = u32_at(0)? as usize;
    let w = u32_at(4)? as i32;
    let h_raw = u32_at(8)? as i32;
    let bpp = u16::from_le_bytes(b.get(14..16)?.try_into().ok()?) as usize;
    let comp = u32_at(16)?;
    let clr_used = u32_at(32)? as usize;
    if bi_size < 40 || w <= 0 || h_raw == 0 || !matches!(bpp, 1 | 4 | 8 | 16 | 24 | 32) || !(comp == 0 || comp == 3) {
        return None;
    }
    let (w, h) = (w as usize, h_raw.unsigned_abs() as usize);
    let bottom_up = h_raw > 0;

    // Channel masks, stored after a 40-byte header or inside a V4/V5 header.
    let (mut rm, mut gm, mut bm, mut am) = (0u32, 0u32, 0u32, 0u32);
    let mut offset = bi_size;
    if comp == 3 {
        rm = u32_at(40)?;
        gm = u32_at(44)?;
        bm = u32_at(48)?;
        if bi_size == 40 {
            offset += 12;
        } else if bi_size >= 56 {
            am = u32_at(52)?;
        }
    }
    if rm == 0 && gm == 0 && bm == 0 {
        (rm, gm, bm) = if bpp == 16 { (0x7C00, 0x03E0, 0x001F) } else { (0x00FF_0000, 0x0000_FF00, 0x0000_00FF) };
    }
    let palette_off = bi_size;
    if bpp <= 8 {
        let n = if clr_used > 0 { clr_used } else { 1 << bpp };
        offset = bi_size + n * 4;
    }

    let stride = (w * bpp).div_ceil(32) * 4;
    if b.len() < offset + stride * h {
        return None;
    }

    let pixel = |sx: usize, row: usize| -> (u8, u8, u8, u8) {
        let base = offset + row * stride;
        match bpp {
            1 | 4 | 8 => {
                let bit = sx * bpp;
                let shift = 8 - bpp - (bit % 8);
                let idx = ((b[base + bit / 8] >> shift) as usize) & ((1 << bpp) - 1);
                let p = palette_off + idx * 4;
                if p + 2 < b.len() { (b[p + 2], b[p + 1], b[p], 255) } else { (0, 0, 0, 255) }
            }
            16 => {
                let v = u16::from_le_bytes([b[base + sx * 2], b[base + sx * 2 + 1]]) as u32;
                (channel(v, rm), channel(v, gm), channel(v, bm), 255)
            }
            24 => {
                let s = base + sx * 3;
                (b[s + 2], b[s + 1], b[s], 255)
            }
            _ => {
                let s = base + sx * 4;
                let v = u32::from_le_bytes([b[s], b[s + 1], b[s + 2], b[s + 3]]);
                let a = if am != 0 { channel(v, am) } else { 255 };
                (channel(v, rm), channel(v, gm), channel(v, bm), a)
            }
        }
    };

    let (tw, th) = fit(w, h);
    let mut px = vec![255u8; tw * th * 4];
    for ty in 0..th {
        let sy = ty * h / th;
        let row = if bottom_up { h - 1 - sy } else { sy };
        for tx in 0..tw {
            let (r, g, bl, a) = pixel(tx * w / tw, row);
            let d = (ty * tw + tx) * 4;
            px[d..d + 4].copy_from_slice(&[bl, g, r, a]);
        }
    }

    // Honour transparency only when the image really uses it; an unused alpha
    // channel (all zero) means opaque.
    if am != 0 && px.chunks_exact(4).any(|p| p[3] != 0) {
        for p in px.chunks_exact_mut(4) {
            let a = p[3] as u32;
            for c in &mut p[..3] {
                *c = ((*c as u32 * a) / 255) as u8;
            }
        }
    } else {
        for p in px.chunks_exact_mut(4) {
            p[3] = 255;
        }
    }
    Some((tw as u32, th as u32, px))
}

/// Decode an encoded image (PNG, JPEG, GIF, ...) with the Windows Imaging Component.
pub fn from_encoded(data: &[u8]) -> Option<Thumb> {
    unsafe {
        let factory: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None::<&IUnknown>, CLSCTX_INPROC_SERVER).ok()?;
        let stream = factory.CreateStream().ok()?;
        stream.InitializeFromMemory(data).ok()?;
        let stream: IStream = stream.cast().ok()?;
        let decoder = factory
            .CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand)
            .ok()?;
        let frame = decoder.GetFrame(0).ok()?;
        let (mut w, mut h) = (0u32, 0u32);
        frame.GetSize(&mut w, &mut h).ok()?;
        if w == 0 || h == 0 {
            return None;
        }
        let (tw, th) = fit(w as usize, h as usize);
        let scaler = factory.CreateBitmapScaler().ok()?;
        scaler
            .Initialize(&frame, tw as u32, th as u32, WICBitmapInterpolationModeFant)
            .ok()?;
        let converter = factory.CreateFormatConverter().ok()?;
        converter
            .Initialize(
                &scaler,
                &GUID_WICPixelFormat32bppPBGRA,
                WICBitmapDitherTypeNone,
                None::<&IWICPalette>,
                0.0,
                WICBitmapPaletteTypeCustom,
            )
            .ok()?;
        let mut px = vec![0u8; tw * th * 4];
        converter.CopyPixels(std::ptr::null(), (tw * 4) as u32, &mut px).ok()?;
        Some((tw as u32, th as u32, px))
    }
}

/// The thumbnail Explorer shows for a file, or failing that its icon. An image's
/// thumbnail is generated on the spot; for other files only an already-cached
/// thumbnail is used, so copying a large video never stalls the line.
pub fn from_file(path: &str) -> Option<Thumb> {
    let image = is_image_path(path);
    unsafe {
        let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
        let item: IShellItem = SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None::<&IBindCtx>).ok()?;
        let factory: IShellItemImageFactory = item.cast().ok()?;
        let size = SIZE { cx: THUMB_MAX_W as i32, cy: THUMB_MAX_H as i32 };
        let attempts = if image {
            [SIIGBF_THUMBNAILONLY, SIIGBF_ICONONLY]
        } else {
            [SIIGBF_THUMBNAILONLY | SIIGBF_INCACHEONLY, SIIGBF_ICONONLY]
        };
        for flags in attempts {
            if let Ok(hbm) = factory.GetImage(size, flags) {
                if let Some(t) = hbitmap_pixels(hbm) {
                    return Some(t);
                }
            }
        }
        None
    }
}

/// Read a shell bitmap into top-down BGRA and free it.
unsafe fn hbitmap_pixels(hbm: HBITMAP) -> Option<Thumb> {
    unsafe {
        let mut bm = BITMAP::default();
        let got = GetObjectW(
            HGDIOBJ(hbm.0),
            std::mem::size_of::<BITMAP>() as i32,
            Some(&mut bm as *mut BITMAP as *mut c_void),
        );
        if got == 0 || bm.bmWidth <= 0 || bm.bmHeight <= 0 {
            let _ = DeleteObject(HGDIOBJ(hbm.0));
            return None;
        }
        let (w, h) = (bm.bmWidth as u32, bm.bmHeight as u32);
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w as i32,
                biHeight: -(h as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut px = vec![0u8; (w * h * 4) as usize];
        let hdc = GetDC(None);
        let lines = GetDIBits(hdc, hbm, 0, h, Some(px.as_mut_ptr() as *mut c_void), &mut info, DIB_RGB_COLORS);
        ReleaseDC(None, hdc);
        let _ = DeleteObject(HGDIOBJ(hbm.0));
        if lines == 0 {
            return None;
        }
        // Some handlers leave alpha at zero everywhere: treat that as opaque.
        if px.chunks_exact(4).all(|p| p[3] == 0) {
            for p in px.chunks_exact_mut(4) {
                p[3] = 255;
            }
        }
        Some((w, h, px))
    }
}
