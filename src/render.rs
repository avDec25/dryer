//! Direct2D drawing into a DirectComposition-bound swap chain.
//! All geometry is in device-independent pixels (DIPs); the context DPI maps them to pixels.

use std::cell::RefCell;
use std::ffi::c_void;

use windows::Win32::Foundation::{HMODULE, HWND};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D_SIZE_U, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_FIGURE_BEGIN_HOLLOW,
    D2D1_FIGURE_BEGIN_FILLED, D2D1_FIGURE_END_CLOSED, D2D1_FIGURE_END_OPEN, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_BITMAP_OPTIONS_CANNOT_DRAW, D2D1_BITMAP_OPTIONS_TARGET, D2D1_BITMAP_PROPERTIES1,
    D2D1_DEVICE_CONTEXT_OPTIONS_NONE, D2D1_DRAW_TEXT_OPTIONS_NO_SNAP, D2D1_DRAW_TEXT_OPTIONS_NONE,
    D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC, D2D1_ROUNDED_RECT, D2D1CreateFactory, ID2D1Bitmap1,
    ID2D1Brush, ID2D1Device, ID2D1PathGeometry1, ID2D1DeviceContext, ID2D1Factory1, ID2D1Image, ID2D1SolidColorBrush,
    ID2D1StrokeStyle,
};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION, D3D11CreateDevice, ID3D11Device,
};
use windows::Win32::Graphics::DirectComposition::{
    DCompositionCreateDevice, IDCompositionDevice, IDCompositionTarget, IDCompositionVisual,
};
use windows::Win32::Graphics::DirectWrite::{DWRITE_TEXT_METRICS, IDWriteTextLayout};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_UNKNOWN,
    DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    DXGI_PRESENT, DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_FLAG, DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
    DXGI_USAGE_RENDER_TARGET_OUTPUT, IDXGIDevice, IDXGIFactory2, IDXGISurface, IDXGISwapChain1,
};
use windows::Win32::Graphics::Direct2D::Common::D2D1_COMPOSITE_MODE_SOURCE_OVER;
use windows::Win32::Graphics::Direct2D::{
    CLSID_D2D1Shadow, D2D1_INTERPOLATION_MODE_LINEAR, D2D1_PROPERTY_TYPE_UNKNOWN,
    D2D1_SHADOW_PROP_BLUR_STANDARD_DEVIATION, D2D1_SHADOW_PROP_COLOR,
};
use windows::core::{Interface, Result};
use windows_numerics::{Matrix3x2, Vector2};

use crate::card::{
    CARD_W, FILE_THUMB_H, INSET, STRIP_H, TEXT_PAD, TIME_H, TOAST_H, slot_x,
    text_top,
};
use crate::clipboard::{Clip, Kind};
use crate::physics::{ROPE_SEGMENTS, Rope};
use crate::ring::Ring;

/// Height of the rope's fixed ends below the top of the screen.
pub const ROPE_Y: f32 = 12.0;
/// From the rope (where the clip bites) down to the top of the card.
pub const PEG_DROP: f32 = 8.0;
/// Margin around a card in its sprite, so the shadow fits.
const SPRITE_PAD: f32 = 46.0;
/// The layers of a card's shadow, widest first: (blur radius, opacity, drop), in DIPs.
/// A wide soft ambient shadow, a medium one under the card, and a tight contact
/// shadow along its edge, as a photo editor would build a realistic drop shadow.
const SHADOW_LAYERS: [(f32, f32, f32); 3] = [(11.0, 0.40, 10.0), (4.5, 0.32, 3.5), (1.3, 0.38, 1.0)];
const FRAME_R: f32 = 13.0;
const PANEL_R: f32 = 9.0;

const FRAME: D2D1_COLOR_F = rgba(0.17, 0.17, 0.20, 0.82);
const FRAME_EDGE: D2D1_COLOR_F = rgba(1.0, 1.0, 1.0, 0.22);
const PANEL: D2D1_COLOR_F = rgba(0.10, 0.10, 0.12, 0.97);
/// The newest card is outlined in this accent colour, with a faint glow around it.
const NEWEST_EDGE: D2D1_COLOR_F = rgba(0.38, 0.68, 1.0, 1.0);
const NEWEST_GLOW: D2D1_COLOR_F = rgba(0.38, 0.68, 1.0, 0.24);
const NOTE_STRIP: D2D1_COLOR_F = rgba(0.98, 0.80, 0.22, 1.0);
const TOAST_FILL: D2D1_COLOR_F = rgba(0.10, 0.10, 0.12, 0.94);
const TOAST_TICK: D2D1_COLOR_F = rgba(0.20, 0.78, 0.40, 1.0);
/// Gap between a card's bottom edge and its toast.
const TOAST_GAP: f32 = 12.0;
/// The toast's shadow layers, widest first: (blur radius, opacity, drop), in DIPs,
/// built like a card's but smaller; and the margin of its bitmap around the toast.
const TOAST_SHADOW_LAYERS: [(f32, f32, f32); 3] = [(7.0, 0.38, 6.0), (3.0, 0.30, 2.0), (1.0, 0.34, 0.7)];
const TOAST_SHADOW_PAD: f32 = 26.0;
const TOAST_TICK_D: f32 = 15.0;
const TOAST_TICK_GAP: f32 = 6.0;
/// Space between the toast's edge and its contents, each side.
const TOAST_PAD: f32 = 13.0;
/// Squircle exponent of the corners: 2 is a circular arc, higher squares it off.
const SQUIRCLE_N: f32 = 4.0;
/// Radius of the toast's corners, and how many points trace each one.
const TOAST_CORNER: f32 = 11.0;
const CORNER_STEPS: usize = 20;
const TEXT: D2D1_COLOR_F = rgba(0.93, 0.93, 0.95, 1.0);
const MUTED: D2D1_COLOR_F = rgba(0.60, 0.60, 0.66, 1.0);
const FOLDER: D2D1_COLOR_F = rgba(0.33, 0.62, 0.96, 1.0);
const FOLDER_TAB: D2D1_COLOR_F = rgba(0.27, 0.53, 0.88, 1.0);
const DOC: D2D1_COLOR_F = rgba(0.55, 0.60, 0.72, 1.0);
const ROPE: D2D1_COLOR_F = rgba(0.93, 0.93, 0.95, 0.92);
const ROPE_SHADOW: D2D1_COLOR_F = rgba(0.0, 0.0, 0.0, 0.35);
const CLIP_BODY: D2D1_COLOR_F = rgba(0.80, 0.81, 0.84, 1.0);
const CLIP_LIGHT: D2D1_COLOR_F = rgba(0.95, 0.96, 0.98, 1.0);
const CLIP_SEAM: D2D1_COLOR_F = rgba(0.45, 0.46, 0.50, 1.0);
const CLIP_SPRING: D2D1_COLOR_F = rgba(0.52, 0.53, 0.58, 1.0);

const fn rgba(r: f32, g: f32, b: f32, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r, g, b, a }
}

fn pt(x: f32, y: f32) -> Vector2 {
    Vector2 { X: x, Y: y }
}

fn rrect(x: f32, y: f32, w: f32, h: f32, r: f32) -> D2D1_ROUNDED_RECT {
    D2D1_ROUNDED_RECT {
        rect: D2D_RECT_F { left: x, top: y, right: x + w, bottom: y + h },
        radiusX: r,
        radiusY: r,
    }
}

/// Scale by `s` and rotate by `theta` radians (clockwise) about (cx, cy).
fn hinge_transform(theta: f32, s: f32, cx: f32, cy: f32) -> Matrix3x2 {
    let (sin, cos) = theta.sin_cos();
    let (m11, m12, m21, m22) = (s * cos, s * sin, -s * sin, s * cos);
    Matrix3x2 {
        M11: m11,
        M12: m12,
        M21: m21,
        M22: m22,
        M31: cx - (cx * m11 + cy * m21),
        M32: cy - (cx * m12 + cy * m22),
    }
}

/// Height of the rope at horizontal position `x`.
pub fn rope_y(x: f32, width: f32, rope: &Rope) -> f32 {
    ROPE_Y + rope.height(x / width)
}

/// Centre x of the card in ring slot `i` of `n`.
pub fn ring_cx(clip: &Clip, i: usize, n: usize, width: f32) -> f32 {
    slot_x(i, n, width) + clip.motion.hx.x
}

/// Where the card's clip grips, in window coordinates: on the rope, which `slide`
/// shifts up out of view, or between there and its source while flying in.
fn hinge_y(clip: &Clip, cx: f32, width: f32, rope: &Rope, slide: f32) -> f32 {
    rope_y(cx, width, rope) + clip.motion.drop_y(slide)
}

/// The flight height that puts the centre of a card, `grow` larger than normal,
/// at screen height `source_y` when the rope there is at `rope_y`.
pub fn rise_to(source_y: f32, rope_y: f32, card_h: f32, grow: f32) -> f32 {
    source_y - (PEG_DROP + card_h / 2.0) * (1.0 + grow) - rope_y
}

/// Which card (if any) is under the point. The newest wins where cards overlap.
pub fn hit(ring: &Ring, rope: &Rope, width: f32, slide: f32, x: f32, y: f32) -> Option<usize> {
    let n = ring.len();
    (0..n).find(|&i| {
        let Some(clip) = ring.get(i) else { return false };
        let cx = ring_cx(clip, i, n, width);
        let top = hinge_y(clip, cx, width, rope, slide) + PEG_DROP;
        x >= cx - CARD_W / 2.0 && x <= cx + CARD_W / 2.0 && y >= top && y <= top + clip.card_h
    })
}

pub struct Scene {
    ctx: ID2D1DeviceContext,
    factory: ID2D1Factory1,
    swap: IDXGISwapChain1,
    /// The swap chain's back buffer as a Direct2D target (None while resizing).
    target: Option<ID2D1Bitmap1>,
    dpi: f32,
    /// The toast's shadow bitmap and the toast width it was made for.
    toast_shadow: RefCell<Option<(f32, ID2D1Bitmap1)>>,
    brush: ID2D1SolidColorBrush,
    // Held so the composition tree and devices stay alive.
    _dcomp: IDCompositionDevice,
    _target: IDCompositionTarget,
    _visual: IDCompositionVisual,
    _d2d: ID2D1Device,
    _d3d: ID3D11Device,
}

impl Scene {
    pub fn new(hwnd: HWND, dpi: u32, width_px: i32, height_px: i32) -> Result<Self> {
        unsafe {
            let mut device: Option<ID3D11Device> = None;
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                None,
            )?;
            let d3d = device.expect("D3D11CreateDevice returned no device");
            let dxgi: IDXGIDevice = d3d.cast()?;

            let factory: ID2D1Factory1 = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let d2d = factory.CreateDevice(&dxgi)?;
            let ctx = d2d.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
            ctx.SetDpi(dpi as f32, dpi as f32);

            let adapter = dxgi.GetAdapter()?;
            let dxgi_factory: IDXGIFactory2 = adapter.GetParent()?;
            let desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: width_px as u32,
                Height: height_px as u32,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 2,
                Scaling: DXGI_SCALING_STRETCH,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
                AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
                ..Default::default()
            };
            let swap = dxgi_factory.CreateSwapChainForComposition(&d3d, &desc, None)?;

            let surface: IDXGISurface = swap.GetBuffer(0)?;
            let props = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: dpi as f32,
                dpiY: dpi as f32,
                bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
                ..Default::default()
            };
            let target = ctx.CreateBitmapFromDxgiSurface(&surface, Some(&props))?;
            ctx.SetTarget(&target);

            let dcomp: IDCompositionDevice = DCompositionCreateDevice(&dxgi)?;
            let comp_target = dcomp.CreateTargetForHwnd(hwnd, true)?;
            let visual = dcomp.CreateVisual()?;
            visual.SetContent(&swap)?;
            comp_target.SetRoot(&visual)?;
            dcomp.Commit()?;

            let brush = ctx.CreateSolidColorBrush(&TEXT, None)?;

            Ok(Self {
                ctx,
                factory,
                swap,
                target: Some(target),
                dpi: dpi as f32,
                toast_shadow: RefCell::new(None),
                brush,
                _dcomp: dcomp,
                _target: comp_target,
                _visual: visual,
                _d2d: d2d,
                _d3d: d3d,
            })
        }
    }

    /// Resize the swap chain to match the window, keeping the device and every
    /// cached card bitmap. The next frame must be drawn afterwards.
    pub fn resize(&mut self, width_px: i32, height_px: i32) -> Result<()> {
        unsafe {
            // Nothing may still reference the old back buffer.
            self.ctx.SetTarget(None::<&ID2D1Image>);
            self.target = None;
            self.swap.ResizeBuffers(
                0,
                width_px as u32,
                height_px as u32,
                DXGI_FORMAT_UNKNOWN,
                DXGI_SWAP_CHAIN_FLAG(0),
            )?;
            let surface: IDXGISurface = self.swap.GetBuffer(0)?;
            let props = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: self.dpi,
                dpiY: self.dpi,
                bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
                ..Default::default()
            };
            let target = self.ctx.CreateBitmapFromDxgiSurface(&surface, Some(&props))?;
            self.ctx.SetTarget(&target);
            self.target = Some(target);
        }
        Ok(())
    }

    /// Draw one frame. `width` is the window width in DIPs. Clips in `departing`
    /// use an absolute x (`hx`) and are drawn behind the line. `slide` raises the
    /// line (negative is up); cards in flight blend into it as they land.
    /// `toast` is the card (by hash) to show "Copied" under, its opacity and its label.
    pub fn draw(
        &self,
        ring: &mut Ring,
        departing: &mut [Clip],
        rope: &Rope,
        width: f32,
        slide: f32,
        toast: Option<(u64, f32, &IDWriteTextLayout)>,
    ) -> Result<()> {
        unsafe {
            // Render any new cards into sprites first, then draw the frame.
            for clip in departing.iter_mut() {
                self.ensure_sprite(clip)?;
            }
            for i in 0..ring.len() {
                if let Some(clip) = ring.get_mut(i) {
                    self.ensure_sprite(clip)?;
                }
            }
            if let Some((_, _, label)) = toast {
                self.ensure_toast_shadow(label)?;
            }
            if let Some(target) = self.target.as_ref() {
                self.ctx.SetTarget(target);
            }

            let clear = rgba(0.0, 0.0, 0.0, 0.0);
            self.ctx.BeginDraw();
            self.ctx.Clear(Some(&clear as *const D2D1_COLOR_F));
            self.ctx.SetTransform(&Matrix3x2::identity());

            self.draw_rope(width, rope, slide)?;

            for clip in departing.iter_mut() {
                let cx = clip.motion.hx.x;
                self.draw_card(clip, cx, width, rope, slide)?;
            }
            let n = ring.len();
            // Oldest first, so the newest sits on top.
            for i in (0..n).rev() {
                if let Some(clip) = ring.get_mut(i) {
                    let cx = ring_cx(clip, i, n, width);
                    self.draw_card(clip, cx, width, rope, slide)?;
                }
            }

            self.ctx.SetTransform(&Matrix3x2::identity());
            if let Some((hash, alpha, label)) = toast {
                for i in 0..n {
                    let Some(clip) = ring.get(i) else { continue };
                    if clip.hash == hash {
                        let cx = ring_cx(clip, i, n, width);
                        let top = hinge_y(clip, cx, width, rope, slide) + PEG_DROP;
                        self.draw_toast(cx, top + clip.card_h + TOAST_GAP, alpha, label)?;
                        break;
                    }
                }
            }
            self.ctx.EndDraw(None, None)?;
            self.swap.Present(1, DXGI_PRESENT(0)).ok()?;
            Ok(())
        }
    }

    /// Width of the toast's contents: the tick, a gap and the text.
    fn toast_group_w(label: &IDWriteTextLayout) -> Result<f32> {
        let mut m = DWRITE_TEXT_METRICS::default();
        unsafe { label.GetMetrics(&mut m)? };
        Ok(TOAST_TICK_D + TOAST_TICK_GAP + m.widthIncludingTrailingWhitespace)
    }

    /// The toast's outline, centred at (mx, my), `a` and `b` being half its width
    /// and height: straight sides with quarter-superellipse (squircle) corners
    /// |x/r|^n + |y/r|^n = 1, traced clockwise from the top right.
    fn toast_geometry(&self, mx: f32, my: f32, a: f32, b: f32) -> Result<ID2D1PathGeometry1> {
        let r = TOAST_CORNER;
        let e = 2.0 / SQUIRCLE_N;
        let centres = [
            (mx + a - r, my - b + r),
            (mx + a - r, my + b - r),
            (mx - a + r, my + b - r),
            (mx - a + r, my - b + r),
        ];
        let mut pts: Vec<Vector2> = Vec::new();
        for (q, (ccx, ccy)) in centres.into_iter().enumerate() {
            for k in 0..=CORNER_STEPS {
                let t = (q as f32 - 1.0 + k as f32 / CORNER_STEPS as f32) * std::f32::consts::FRAC_PI_2;
                let (s, c) = t.sin_cos();
                pts.push(pt(ccx + r * c.signum() * c.abs().powf(e), ccy + r * s.signum() * s.abs().powf(e)));
            }
        }
        unsafe {
            let geom = self.factory.CreatePathGeometry()?;
            let sink = geom.Open()?;
            sink.BeginFigure(pts[0], D2D1_FIGURE_BEGIN_FILLED);
            sink.AddLines(&pts[1..]);
            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
            sink.Close()?;
            Ok(geom)
        }
    }

    /// Render the toast's shadow once into a bitmap, the way a card's is made:
    /// real Gaussian blurs of its silhouette. Rebuilt only if the toast's width
    /// changes. Must run outside a draw.
    fn ensure_toast_shadow(&self, label: &IDWriteTextLayout) -> Result<()> {
        let w = Self::toast_group_w(label)? + 2.0 * TOAST_PAD;
        if self.toast_shadow.borrow().as_ref().is_some_and(|(cached, _)| (*cached - w).abs() < 0.01) {
            return Ok(());
        }
        let (bw, bh) = (w + 2.0 * TOAST_SHADOW_PAD, TOAST_H + 2.0 * TOAST_SHADOW_PAD);
        let s = self.dpi / 96.0;
        let props = D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: self.dpi,
            dpiY: self.dpi,
            bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET,
            ..Default::default()
        };
        let size = D2D_SIZE_U { width: (bw * s).ceil() as u32, height: (bh * s).ceil() as u32 };
        let geom = self.toast_geometry(bw / 2.0, bh / 2.0, w / 2.0, TOAST_H / 2.0)?;
        unsafe {
            let clear = rgba(0.0, 0.0, 0.0, 0.0);
            let mask = self.ctx.CreateBitmap(size, None, 0, &props)?;
            self.ctx.SetTarget(&mask);
            self.ctx.BeginDraw();
            self.ctx.Clear(Some(&clear as *const D2D1_COLOR_F));
            self.ctx.SetTransform(&Matrix3x2::identity());
            self.set(rgba(1.0, 1.0, 1.0, 1.0));
            self.ctx.FillGeometry(&geom, &self.brush, None::<&ID2D1Brush>);
            self.ctx.EndDraw(None, None)?;

            let mut shadows = Vec::new();
            for (sigma, alpha, drop) in TOAST_SHADOW_LAYERS {
                let effect = self.ctx.CreateEffect(&CLSID_D2D1Shadow)?;
                effect.SetInput(0, &mask, true);
                effect.SetValue(
                    D2D1_SHADOW_PROP_BLUR_STANDARD_DEVIATION.0 as u32,
                    D2D1_PROPERTY_TYPE_UNKNOWN,
                    &sigma.to_ne_bytes(),
                )?;
                let colour: Vec<u8> = [0.0f32, 0.0, 0.0, alpha].iter().flat_map(|c| c.to_ne_bytes()).collect();
                effect.SetValue(D2D1_SHADOW_PROP_COLOR.0 as u32, D2D1_PROPERTY_TYPE_UNKNOWN, &colour)?;
                shadows.push((effect, drop));
            }

            let sprite = self.ctx.CreateBitmap(size, None, 0, &props)?;
            self.ctx.SetTarget(&sprite);
            self.ctx.BeginDraw();
            self.ctx.Clear(Some(&clear as *const D2D1_COLOR_F));
            self.ctx.SetTransform(&Matrix3x2::identity());
            for (effect, drop) in &shadows {
                let image = effect.GetOutput()?;
                let offset = Vector2 { X: 0.0, Y: *drop };
                let _ = self.ctx.DrawImage(
                    &image,
                    Some(&offset as *const Vector2),
                    None,
                    D2D1_INTERPOLATION_MODE_LINEAR,
                    D2D1_COMPOSITE_MODE_SOURCE_OVER,
                );
            }
            self.ctx.EndDraw(None, None)?;
            *self.toast_shadow.borrow_mut() = Some((w, sprite));
        }
        Ok(())
    }

    /// A small squircle toast (green tick, then the label), top-centre at (cx, y).
    fn draw_toast(&self, cx: f32, y: f32, alpha: f32, label: &IDWriteTextLayout) -> Result<()> {
        let fade = |c: D2D1_COLOR_F| rgba(c.r, c.g, c.b, c.a * alpha);
        // Tick and text sit side by side, centred as a group in a box sized to fit.
        let group_w = Self::toast_group_w(label)?;
        let a = (group_w + 2.0 * TOAST_PAD) / 2.0;
        let b = TOAST_H / 2.0;
        let (mx, my) = (cx, y + b);
        let geom = self.toast_geometry(mx, my, a, b)?;
        unsafe {
            if let Some((_, shadow)) = self.toast_shadow.borrow().as_ref() {
                let dest = D2D_RECT_F {
                    left: mx - a - TOAST_SHADOW_PAD,
                    top: y - TOAST_SHADOW_PAD,
                    right: mx + a + TOAST_SHADOW_PAD,
                    bottom: y + TOAST_H + TOAST_SHADOW_PAD,
                };
                self.ctx.DrawBitmap(
                    shadow,
                    Some(&dest as *const D2D_RECT_F),
                    alpha,
                    D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC,
                    None,
                    None,
                );
            }

            self.set(fade(TOAST_FILL));
            self.ctx.FillGeometry(&geom, &self.brush, None::<&ID2D1Brush>);
            self.set(fade(FRAME_EDGE));
            self.ctx.DrawGeometry(&geom, &self.brush, 1.0, None::<&ID2D1StrokeStyle>);
        }

        // The green circle with a white tick.
        let start = mx - group_w / 2.0;
        let (tx, ty) = (start + TOAST_TICK_D / 2.0, my);
        let r = TOAST_TICK_D / 2.0;
        self.fill_rr(rrect(tx - r, ty - r, 2.0 * r, 2.0 * r, r), fade(TOAST_TICK));
        let white = fade(rgba(1.0, 1.0, 1.0, 1.0));
        self.line(tx - 3.4, ty + 0.2, tx - 1.1, ty + 2.5, 1.7, white);
        self.line(tx - 1.1, ty + 2.5, tx + 3.4, ty - 2.5, 1.7, white);

        // Drawn unsnapped, so the text glides with the box instead of jumping a pixel at a time.
        self.set(fade(TEXT));
        unsafe {
            self.ctx.DrawTextLayout(
                pt(start + TOAST_TICK_D + TOAST_TICK_GAP, y),
                label,
                &self.brush,
                D2D1_DRAW_TEXT_OPTIONS_NO_SNAP,
            );
        }
        Ok(())
    }

    /// One card hanging from its clip, scaled while flying in and swung by its pendulum.
    /// The card itself is a cached sprite; only the clip is drawn live.
    fn draw_card(&self, clip: &mut Clip, cx: f32, width: f32, rope: &Rope, slide: f32) -> Result<()> {
        let hy = hinge_y(clip, cx, width, rope, slide);
        let top = hy + PEG_DROP;
        let left = cx - CARD_W / 2.0;
        let m = hinge_transform(clip.motion.swing.x, 1.0 + clip.motion.grow.x, cx, hy);
        if let Some(sprite) = clip.sprite.as_ref() {
            let dest = D2D_RECT_F {
                left: left - SPRITE_PAD,
                top: top - SPRITE_PAD,
                right: left + CARD_W + SPRITE_PAD,
                bottom: top + clip.card_h + SPRITE_PAD,
            };
            unsafe {
                self.ctx.SetTransform(&m);
                self.ctx.DrawBitmap(
                    sprite,
                    Some(&dest as *const D2D_RECT_F),
                    1.0,
                    D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC,
                    None,
                    None,
                );
            }
        }
        unsafe { self.ctx.SetTransform(&m) };

        // The newest card is outlined. This is drawn live, not baked into the sprite,
        // because a card stops being the newest as soon as another one arrives; the
        // outline fades in and out with `glow`.
        let glow = clip.motion.glow.x.clamp(0.0, 1.0);
        if glow > 0.01 {
            let h = clip.card_h;
            let faded = |c: D2D1_COLOR_F| rgba(c.r, c.g, c.b, c.a * glow);
            self.stroke_rr(rrect(left - 1.5, top - 1.5, CARD_W + 3.0, h + 3.0, FRAME_R + 1.5), 5.0, faded(NEWEST_GLOW));
            self.stroke_rr(rrect(left + 0.5, top + 0.5, CARD_W - 1.0, h - 1.0, FRAME_R - 0.5), 2.0, faded(NEWEST_EDGE));
        }

        self.clothespin(cx, hy);
        Ok(())
    }

    /// Render the card once into an offscreen bitmap (it never changes), so each
    /// frame only moves and rotates a picture instead of re-rasterising text.
    fn ensure_sprite(&self, clip: &mut Clip) -> Result<()> {
        if clip.sprite.is_some() {
            return Ok(());
        }
        let (w, h) = (CARD_W + 2.0 * SPRITE_PAD, clip.card_h + 2.0 * SPRITE_PAD);
        let s = self.dpi / 96.0;
        let props = D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: self.dpi,
            dpiY: self.dpi,
            bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET,
            ..Default::default()
        };
        let size = D2D_SIZE_U { width: (w * s).ceil() as u32, height: (h * s).ceil() as u32 };
        unsafe {
            let clear = rgba(0.0, 0.0, 0.0, 0.0);

            // 1. The card's silhouette, which the shadows are blurred from.
            let mask = self.ctx.CreateBitmap(size, None, 0, &props)?;
            self.ctx.SetTarget(&mask);
            self.ctx.BeginDraw();
            self.ctx.Clear(Some(&clear as *const D2D1_COLOR_F));
            self.ctx.SetTransform(&Matrix3x2::identity());
            self.fill_rr(
                rrect(SPRITE_PAD, SPRITE_PAD, CARD_W, clip.card_h, FRAME_R),
                rgba(1.0, 1.0, 1.0, 1.0),
            );
            self.ctx.EndDraw(None, None)?;

            // 2. Real Gaussian blurs of it: (blur radius, opacity, how far it drops).
            let mut shadows = Vec::new();
            for (sigma, alpha, drop) in SHADOW_LAYERS {
                let effect = self.ctx.CreateEffect(&CLSID_D2D1Shadow)?;
                effect.SetInput(0, &mask, true);
                effect.SetValue(
                    D2D1_SHADOW_PROP_BLUR_STANDARD_DEVIATION.0 as u32,
                    D2D1_PROPERTY_TYPE_UNKNOWN,
                    &sigma.to_ne_bytes(),
                )?;
                let colour: Vec<u8> = [0.0f32, 0.0, 0.0, alpha].iter().flat_map(|c| c.to_ne_bytes()).collect();
                effect.SetValue(D2D1_SHADOW_PROP_COLOR.0 as u32, D2D1_PROPERTY_TYPE_UNKNOWN, &colour)?;
                shadows.push((effect, drop));
            }

            // 3. The sprite: shadows underneath, then the card itself.
            let sprite = self.ctx.CreateBitmap(size, None, 0, &props)?;
            self.ctx.SetTarget(&sprite);
            self.ctx.BeginDraw();
            self.ctx.Clear(Some(&clear as *const D2D1_COLOR_F));
            self.ctx.SetTransform(&Matrix3x2::identity());
            for (effect, drop) in &shadows {
                let image = effect.GetOutput()?;
                let offset = Vector2 { X: 0.0, Y: *drop };
                let _ = self.ctx.DrawImage(
                    &image,
                    Some(&offset as *const Vector2),
                    None,
                    D2D1_INTERPOLATION_MODE_LINEAR,
                    D2D1_COMPOSITE_MODE_SOURCE_OVER,
                );
            }
            let drawn = self.draw_body(clip, SPRITE_PAD, SPRITE_PAD);
            self.ctx.EndDraw(None, None)?;
            drawn?;
            clip.sprite = Some(sprite);
        }
        // The thumbnail now lives inside the sprite; free its GPU copy.
        clip.thumb = None;
        Ok(())
    }

    /// Shadow, glass frame and content of a card with its top-left at (left, top).
    fn draw_body(&self, clip: &mut Clip, left: f32, top: f32) -> Result<()> {
        let h = clip.card_h;
        let cx = left + CARD_W / 2.0;

        // Glass frame.
        self.fill_rr(rrect(left, top, CARD_W, h, FRAME_R), FRAME);
        self.stroke_rr(rrect(left + 0.5, top + 0.5, CARD_W - 1.0, h - 1.0, FRAME_R), 1.0, FRAME_EDGE);

        // Content panel.
        let (px, py) = (left + INSET, top + INSET);
        let (pw, ph) = (CARD_W - 2.0 * INSET, h - 2.0 * INSET);
        let text_x = px + TEXT_PAD;
        let time_y = top + h - INSET - TEXT_PAD - TIME_H;
        match clip.kind {
            Kind::Image => {
                // The picture fills the whole panel.
                self.fill_rr(rrect(px, py, pw, ph, PANEL_R), PANEL);
                self.image(clip, px, py, pw, ph, true)?;
            }
            Kind::Text => {
                // A notes sheet: yellow strip across the top of the panel.
                self.fill_rr(rrect(px, py, pw, ph, PANEL_R), NOTE_STRIP);
                self.fill_rr(rrect(px, py + STRIP_H, pw, ph - STRIP_H, PANEL_R), PANEL);
                self.fill_rect(px, py + STRIP_H, pw, PANEL_R, PANEL);
                let text_y = top + text_top(clip);
                self.text(clip.layout.as_ref(), text_x, text_y, TEXT);
                self.text(clip.time_layout.as_ref(), text_x, time_y, MUTED);
            }
            Kind::Files | Kind::Other => {
                self.fill_rr(rrect(px, py, pw, ph, PANEL_R), PANEL);
                if clip.thumb_px.is_some() {
                    // The file's thumbnail (or icon), whole, above its name.
                    let (mx, my) = (px + TEXT_PAD, py + TEXT_PAD);
                    self.image(clip, mx, my, pw - 2.0 * TEXT_PAD, FILE_THUMB_H - 8.0, false)?;
                } else if clip.kind == Kind::Files {
                    self.folder(cx, py + TEXT_PAD);
                } else {
                    self.doc_glyph(cx, py + TEXT_PAD);
                }
                let text_y = top + text_top(clip);
                self.text(clip.layout.as_ref(), text_x, text_y, TEXT);
                self.text(clip.time_layout.as_ref(), text_x, time_y, MUTED);
            }
        }
        Ok(())
    }

    fn draw_rope(&self, width: f32, rope: &Rope, slide: f32) -> Result<()> {
        unsafe {
            let pts: Vec<Vector2> = (0..=ROPE_SEGMENTS)
                .map(|i| {
                    let x = width * i as f32 / ROPE_SEGMENTS as f32;
                    pt(x, rope_y(x, width, rope) + slide)
                })
                .collect();

            let geom = self.factory.CreatePathGeometry()?;
            let sink = geom.Open()?;
            sink.BeginFigure(pts[0], D2D1_FIGURE_BEGIN_HOLLOW);
            sink.AddLines(&pts[1..]);
            sink.EndFigure(D2D1_FIGURE_END_OPEN);
            sink.Close()?;

            self.set(ROPE_SHADOW);
            self.ctx.SetTransform(&Matrix3x2::translation(0.0, 1.5));
            self.ctx
                .DrawGeometry(&geom, &self.brush, 2.2, None::<&ID2D1StrokeStyle>);
            self.ctx.SetTransform(&Matrix3x2::identity());
            self.set(ROPE);
            self.ctx
                .DrawGeometry(&geom, &self.brush, 1.3, None::<&ID2D1StrokeStyle>);
            Ok(())
        }
    }

    /// A silver clothespin gripping the rope and the top of the card.
    fn clothespin(&self, cx: f32, hy: f32) {
        self.fill_rr(rrect(cx - 4.5, hy - 8.0, 9.0, 22.0, 2.5), CLIP_BODY);
        self.fill_rr(rrect(cx - 4.5, hy - 8.0, 4.5, 22.0, 2.0), CLIP_LIGHT);
        self.line(cx, hy - 7.0, cx, hy + 13.0, 1.0, CLIP_SEAM);
        self.fill_rr(rrect(cx - 5.5, hy + 1.0, 11.0, 3.5, 1.5), CLIP_SPRING);
    }

    /// A small folder icon centred at `cx`.
    fn folder(&self, cx: f32, y: f32) {
        let (w, h) = (34.0, 26.0);
        let x = cx - w / 2.0;
        self.fill_rr(rrect(x, y, w * 0.45, 8.0, 2.5), FOLDER_TAB);
        self.fill_rr(rrect(x, y + 4.0, w, h - 4.0, 3.5), FOLDER);
    }

    fn text(&self, layout: Option<&IDWriteTextLayout>, x: f32, y: f32, color: D2D1_COLOR_F) {
        let Some(layout) = layout else { return };
        self.set(color);
        unsafe {
            self.ctx
                .DrawTextLayout(pt(x, y), layout, &self.brush, D2D1_DRAW_TEXT_OPTIONS_NONE);
        }
    }

    /// A small document icon for clipboard data that has no picture.
    fn doc_glyph(&self, cx: f32, y: f32) {
        let (w, h) = (30.0, 38.0);
        let x = cx - w / 2.0;
        self.fill_rr(rrect(x, y, w, h, 4.0), DOC);
        for k in 0..3 {
            self.fill_rr(rrect(x + 6.0, y + 10.0 + k as f32 * 9.0, w - 12.0, 3.0, 1.5), PANEL);
        }
    }

    /// The thumbnail inside the box (x, y, w, h), with rounded corners. `cover`
    /// fills the whole box and crops the overflow; otherwise the whole picture is
    /// shown, centred.
    fn image(&self, clip: &mut Clip, x: f32, y: f32, w: f32, h: f32, cover: bool) -> Result<()> {
        if clip.thumb.is_none() {
            if let Some((tw, th, px)) = clip.thumb_px.as_ref() {
                let props = D2D1_BITMAP_PROPERTIES1 {
                    pixelFormat: D2D1_PIXEL_FORMAT {
                        format: DXGI_FORMAT_B8G8R8A8_UNORM,
                        alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                    },
                    dpiX: 96.0,
                    dpiY: 96.0,
                    ..Default::default()
                };
                let bmp: ID2D1Bitmap1 = unsafe {
                    self.ctx.CreateBitmap(
                        D2D_SIZE_U { width: *tw, height: *th },
                        Some(px.as_ptr() as *const c_void),
                        tw * 4,
                        &props,
                    )?
                };
                clip.thumb = Some(bmp);
            }
        }
        let (Some(bmp), Some((tw, th, _))) = (clip.thumb.as_ref(), clip.thumb_px.as_ref()) else {
            return Ok(());
        };
        let (fw, fh) = (w / *tw as f32, h / *th as f32);
        let s = if cover { fw.max(fh) } else { fw.min(fh) };
        let (dw, dh) = (*tw as f32 * s, *th as f32 * s);
        let (dx, dy) = (x + (w - dw) / 2.0, y + (h - dh) / 2.0);
        // Where the picture is painted: the whole box, or just where it lands.
        let dest = if cover { rrect(x, y, w, h, PANEL_R) } else { rrect(dx, dy, dw, dh, 4.0) };
        unsafe {
            let brush = self.ctx.CreateBitmapBrush(bmp, None, None)?;
            brush.SetTransform(&Matrix3x2 { M11: s, M12: 0.0, M21: 0.0, M22: s, M31: dx, M32: dy });
            self.ctx.FillRoundedRectangle(&dest, &brush);
        }
        Ok(())
    }

    fn set(&self, c: D2D1_COLOR_F) {
        unsafe { self.brush.SetColor(&c) }
    }

    fn fill_rr(&self, rr: D2D1_ROUNDED_RECT, c: D2D1_COLOR_F) {
        self.set(c);
        unsafe { self.ctx.FillRoundedRectangle(&rr, &self.brush) }
    }

    fn stroke_rr(&self, rr: D2D1_ROUNDED_RECT, width: f32, c: D2D1_COLOR_F) {
        self.set(c);
        unsafe {
            self.ctx
                .DrawRoundedRectangle(&rr, &self.brush, width, None::<&ID2D1StrokeStyle>)
        }
    }

    fn fill_rect(&self, x: f32, y: f32, w: f32, h: f32, c: D2D1_COLOR_F) {
        self.set(c);
        let r = D2D_RECT_F { left: x, top: y, right: x + w, bottom: y + h };
        unsafe { self.ctx.FillRectangle(&r, &self.brush) }
    }

    fn line(&self, x0: f32, y0: f32, x1: f32, y1: f32, width: f32, c: D2D1_COLOR_F) {
        self.set(c);
        unsafe {
            self.ctx.DrawLine(
                pt(x0, y0),
                pt(x1, y1),
                &self.brush,
                width,
                None::<&ID2D1StrokeStyle>,
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn card_centre_starts_on_the_source() {
        let (source_y, rope, h, g) = (640.0, 19.5, 180.0, 0.25);
        let rise = rise_to(source_y, rope, h, g);
        let centre = rope + rise + (PEG_DROP + h / 2.0) * (1.0 + g);
        assert!((centre - source_y).abs() < 0.001);
    }
}
