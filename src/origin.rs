//! Finds where on screen the copied content came from, so a new card can fly in
//! from that spot. What counts as the source depends on what was copied; the
//! evidence is the recent mouse gestures and what Windows reports about text,
//! images, lists and windows.

use std::ffi::c_void;
use std::mem::size_of;
use std::ptr::null_mut;

use windows::Win32::Foundation::{POINT, RECT};
use windows::Win32::Graphics::Dwm::{DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, SAFEARRAY,
};
use windows::Win32::System::Ole::{
    SafeArrayAccessData, SafeArrayDestroy, SafeArrayGetLBound, SafeArrayGetUBound,
    SafeArrayUnaccessData,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomation6, IUIAutomationElement,
    IUIAutomationSelectionItemPattern, IUIAutomationSelectionPattern, IUIAutomationTextPattern,
    UIA_ImageControlTypeId, UIA_SelectionItemPatternId, UIA_SelectionPatternId, UIA_TextPatternId,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GA_ROOT, GUITHREADINFO, GetAncestor, GetGUIThreadInfo, GetSystemMetrics, SM_CXVIRTUALSCREEN,
    SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, WindowFromPoint,
};
use windows::core::{IUnknown, Interface};

use crate::clipboard::Copied;
use crate::gesture::{Gestures, Point};

/// How long to wait on a slow or hung application before giving up (ms).
const UIA_TIMEOUT_MS: u32 = 150;
/// A snip reaches the clipboard this soon after the drag or click that made it (ms).
const SNIP_MS: u64 = 5_000;
/// A click this recent was on whatever did the copying, such as a "Copy" button (ms).
const BUTTON_CLICK_MS: u64 = 1_500;
/// How long a drag can still be the selection that is being copied (ms).
const TEXT_DRAG_MS: u64 = 30_000;
/// How far a captured image's size may differ from the area it was taken from (px).
const SIZE_SLACK: f64 = 4.0;
/// Most selected ranges or list items looked at; each one is a call to the other app.
const MAX_RANGES: i32 = 16;
const MAX_ITEMS: i32 = 32;
/// Most rectangles (one per line of text) read from one selected range.
const MAX_RECTS: usize = 256;

/// A screen position in physical pixels.
pub type Pos = (f32, f32);
/// A screen rectangle in physical pixels: left, top, right, bottom.
pub type Rect = [f64; 4];

/// What the operating system can tell about the screen right now. Each question is
/// only asked if the answer is needed.
pub trait Probe {
    /// Centre of the focused element's selected text.
    fn selection(&self) -> Option<Pos>;
    /// Bounds of the top-level window under a point.
    fn window_at(&self, p: Point) -> Option<Rect>;
    /// Bounds of the picture under a point, if there is a picture there.
    fn image_at(&self, p: Point) -> Option<Rect>;
    /// Centre of the selected items of the focused list.
    fn selected_items(&self) -> Option<Pos>;
    /// The text caret of the foreground window.
    fn caret(&self) -> Option<Pos>;
    /// Centre of the focused control.
    fn focused(&self) -> Option<Pos>;
}

fn centre(r: Rect) -> Pos {
    (((r[0] + r[2]) / 2.0) as f32, ((r[1] + r[3]) / 2.0) as f32)
}

fn point_pos(p: Point) -> Pos {
    (p.0 as f32, p.1 as f32)
}

fn drag_rect((a, b): (Point, Point)) -> Rect {
    [a.0.min(b.0) as f64, a.1.min(b.1) as f64, a.0.max(b.0) as f64, a.1.max(b.1) as f64]
}

fn same_size(r: Rect, (w, h): (u32, u32)) -> bool {
    (r[2] - r[0] - w as f64).abs() <= SIZE_SLACK && (r[3] - r[1] - h as f64).abs() <= SIZE_SLACK
}

/// The box around the parts of `rects` that lie on `screen`; empty and fully
/// off-screen rectangles are ignored.
pub fn union_visible(rects: impl IntoIterator<Item = Rect>, screen: Rect) -> Option<Rect> {
    rects
        .into_iter()
        .map(|r| {
            [r[0].max(screen[0]), r[1].max(screen[1]), r[2].min(screen[2]), r[3].min(screen[3])]
        })
        .filter(|r| r[2] > r[0] && r[3] > r[1])
        .reduce(|a, b| [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])])
}

/// Where the copy came from: the first piece of evidence that fits what was
/// copied. None if there is none; the caller then uses the pointer.
pub fn choose(
    copied: Copied,
    g: &Gestures,
    now: u64,
    monitor: Rect,
    probe: &dyn Probe,
) -> Option<Pos> {
    let right = || g.right_click(now).map(point_pos);
    match copied {
        Copied::Image(size) => {
            // A region snip: the picture is as big as the rectangle just dragged.
            let snipped = size.and_then(|size| {
                g.drag(now, SNIP_MS).map(drag_rect).filter(|&r| same_size(r, size))
            });
            // A window snip: as big as the window just clicked.
            let window = || {
                let size = size?;
                let click = g.left_click(now, SNIP_MS)?;
                probe.window_at(click).filter(|&r| same_size(r, size))
            };
            // "Copy image": the picture that was right-clicked.
            let picked = || {
                let p = g.right_click(now)?;
                Some(probe.image_at(p).map_or(point_pos(p), centre))
            };
            // A capture of the whole screen.
            let full = || {
                let (w, h) = size?;
                let covers = w as f64 >= monitor[2] - monitor[0] - SIZE_SLACK
                    && h as f64 >= monitor[3] - monitor[1] - SIZE_SLACK;
                covers.then(|| centre(monitor))
            };
            snipped.or_else(window).map(centre).or_else(picked).or_else(full)
        }
        Copied::Files => right()
            .or_else(|| probe.selected_items())
            .or_else(|| probe.focused()),
        Copied::Other => probe
            .selection()
            .or_else(|| g.left_click(now, BUTTON_CLICK_MS).map(point_pos))
            .or_else(right)
            .or_else(|| g.drag(now, TEXT_DRAG_MS).map(|d| centre(drag_rect(d))))
            .or_else(|| probe.caret())
            .or_else(|| probe.focused()),
    }
}

pub struct Locator {
    uia: Option<IUIAutomation>,
}

impl Locator {
    pub fn new() -> Self {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let uia: Option<IUIAutomation> =
                CoCreateInstance(&CUIAutomation, None::<&IUnknown>, CLSCTX_INPROC_SERVER).ok();
            if let Some(u6) = uia.as_ref().and_then(|u| u.cast::<IUIAutomation6>().ok()) {
                // Never let a stuck application freeze the line.
                let _ = u6.SetConnectionTimeout(UIA_TIMEOUT_MS);
                let _ = u6.SetTransactionTimeout(UIA_TIMEOUT_MS);
            }
            Self { uia }
        }
    }

    fn focused_element(&self) -> Option<IUIAutomationElement> {
        unsafe { self.uia.as_ref()?.GetFocusedElement().ok() }
    }
}

impl Probe for Locator {
    fn selection(&self) -> Option<Pos> {
        let el = self.focused_element()?;
        unsafe { selection_rect(&el) }.map(centre)
    }

    fn window_at(&self, p: Point) -> Option<Rect> {
        unsafe {
            let hit = WindowFromPoint(POINT { x: p.0, y: p.1 });
            if hit.is_invalid() {
                return None;
            }
            // The visible frame, without the invisible resize border.
            let mut r = RECT::default();
            DwmGetWindowAttribute(
                GetAncestor(hit, GA_ROOT),
                DWMWA_EXTENDED_FRAME_BOUNDS,
                &mut r as *mut RECT as *mut c_void,
                size_of::<RECT>() as u32,
            )
            .ok()?;
            Some(to_rect(r))
        }
    }

    fn image_at(&self, p: Point) -> Option<Rect> {
        unsafe {
            let el = self.uia.as_ref()?.ElementFromPoint(POINT { x: p.0, y: p.1 }).ok()?;
            if el.CurrentControlType().ok()? != UIA_ImageControlTypeId {
                return None;
            }
            element_rect(&el)
        }
    }

    fn selected_items(&self) -> Option<Pos> {
        let el = self.focused_element()?;
        unsafe { selected_rect(&el) }.map(centre)
    }

    fn caret(&self) -> Option<Pos> {
        unsafe { caret_center() }
    }

    fn focused(&self) -> Option<Pos> {
        let el = self.focused_element()?;
        unsafe { element_rect(&el) }.map(centre)
    }
}

fn to_rect(r: RECT) -> Rect {
    [r.left as f64, r.top as f64, r.right as f64, r.bottom as f64]
}

/// The bounds of every monitor together.
fn virtual_screen() -> Rect {
    unsafe {
        let (x, y) = (GetSystemMetrics(SM_XVIRTUALSCREEN), GetSystemMetrics(SM_YVIRTUALSCREEN));
        let (w, h) = (GetSystemMetrics(SM_CXVIRTUALSCREEN), GetSystemMetrics(SM_CYVIRTUALSCREEN));
        to_rect(RECT { left: x, top: y, right: x + w, bottom: y + h })
    }
}

/// The box around the visible lines of the element's selected text.
unsafe fn selection_rect(el: &IUIAutomationElement) -> Option<Rect> {
    unsafe {
        let pattern: IUIAutomationTextPattern = el.GetCurrentPatternAs(UIA_TextPatternId).ok()?;
        let selection = pattern.GetSelection().ok()?;
        let mut lines = Vec::new();
        for i in 0..selection.Length().ok()?.min(MAX_RANGES) {
            let Ok(range) = selection.GetElement(i) else { continue };
            let Ok(array) = range.GetBoundingRectangles() else { continue };
            lines.extend(take_rects(array));
        }
        union_visible(lines, virtual_screen())
    }
}

/// The rectangles in a UI Automation array of (left, top, width, height) values.
/// Takes ownership of, and frees, the array.
unsafe fn take_rects(sa: *mut SAFEARRAY) -> Vec<Rect> {
    unsafe {
        let mut out = Vec::new();
        if sa.is_null() {
            return out;
        }
        if let (Ok(lower), Ok(upper)) = (SafeArrayGetLBound(sa, 1), SafeArrayGetUBound(sa, 1)) {
            let count = ((upper - lower + 1).max(0) as usize / 4).min(MAX_RECTS);
            let mut data: *mut c_void = null_mut();
            if SafeArrayAccessData(sa, &mut data).is_ok() {
                let d = data as *const f64;
                for i in 0..count {
                    let [l, t, w, h] = *(d.add(4 * i) as *const [f64; 4]);
                    out.push([l, t, l + w, t + h]);
                }
                let _ = SafeArrayUnaccessData(sa);
            }
        }
        let _ = SafeArrayDestroy(sa);
        out
    }
}

/// The box around the selected items of a list: `el` is either one of those items
/// (as in Explorer) or the list itself.
unsafe fn selected_rect(el: &IUIAutomationElement) -> Option<Rect> {
    unsafe {
        let item: windows::core::Result<IUIAutomationSelectionItemPattern> =
            el.GetCurrentPatternAs(UIA_SelectionItemPatternId);
        let list = match item {
            Ok(item) => item.CurrentSelectionContainer().ok()?,
            Err(_) => el.clone(),
        };
        let pattern: IUIAutomationSelectionPattern =
            list.GetCurrentPatternAs(UIA_SelectionPatternId).ok()?;
        let selected = pattern.GetCurrentSelection().ok()?;
        let mut items = Vec::new();
        for i in 0..selected.Length().ok()?.min(MAX_ITEMS) {
            if let Some(r) = selected.GetElement(i).ok().and_then(|e| element_rect(&e)) {
                items.push(r);
            }
        }
        union_visible(items, virtual_screen())
    }
}

/// Centre of the system text caret of the foreground window, for classic controls.
unsafe fn caret_center() -> Option<Pos> {
    unsafe {
        let mut info = GUITHREADINFO {
            cbSize: size_of::<GUITHREADINFO>() as u32,
            ..Default::default()
        };
        GetGUIThreadInfo(0, &mut info).ok()?;
        if info.hwndCaret.is_invalid() {
            return None;
        }
        let r = info.rcCaret;
        if r.right - r.left <= 0 && r.bottom - r.top <= 0 {
            return None;
        }
        let mut p = POINT { x: (r.left + r.right) / 2, y: (r.top + r.bottom) / 2 };
        if !ClientToScreen(info.hwndCaret, &mut p).as_bool() {
            return None;
        }
        Some((p.x as f32, p.y as f32))
    }
}

/// The element's bounds, if it takes up any room.
unsafe fn element_rect(el: &IUIAutomationElement) -> Option<Rect> {
    unsafe {
        let r = el.CurrentBoundingRectangle().ok()?;
        (r.right > r.left && r.bottom > r.top).then(|| to_rect(r))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MONITOR: Rect = [0.0, 0.0, 1920.0, 1080.0];

    /// What the operating system would answer.
    #[derive(Default)]
    struct Fake {
        selection: Option<Pos>,
        window: Option<Rect>,
        image: Option<Rect>,
        items: Option<Pos>,
        caret: Option<Pos>,
        focused: Option<Pos>,
    }

    impl Probe for Fake {
        fn selection(&self) -> Option<Pos> {
            self.selection
        }
        fn window_at(&self, _: Point) -> Option<Rect> {
            self.window
        }
        fn image_at(&self, _: Point) -> Option<Rect> {
            self.image
        }
        fn selected_items(&self) -> Option<Pos> {
            self.items
        }
        fn caret(&self) -> Option<Pos> {
            self.caret
        }
        fn focused(&self) -> Option<Pos> {
            self.focused
        }
    }

    fn drag(g: &mut Gestures, from: Point, to: Point, now: u64) {
        g.left_down(from);
        g.left_up(to, now);
    }

    fn pick(copied: Copied, g: &Gestures, now: u64, probe: &Fake) -> Option<Pos> {
        choose(copied, g, now, MONITOR, probe)
    }

    #[test]
    fn a_region_snip_starts_at_the_dragged_rectangle_in_either_direction() {
        for (from, to) in [((100, 200), (500, 400)), ((500, 400), (100, 200))] {
            let mut g = Gestures::default();
            drag(&mut g, from, to, 1_000);
            let got = pick(Copied::Image(Some((401, 199))), &g, 2_000, &Fake::default());
            assert_eq!(got, Some((300.0, 300.0)));
        }
    }

    #[test]
    fn a_drag_of_another_size_or_too_old_is_not_the_snip() {
        let mut g = Gestures::default();
        drag(&mut g, (100, 200), (500, 400), 1_000);
        let probe = Fake::default();
        assert_eq!(pick(Copied::Image(Some((405, 200))), &g, 2_000, &probe), None);
        assert_eq!(pick(Copied::Image(Some((400, 200))), &g, 6_001, &probe), None);
    }

    #[test]
    fn a_window_snip_starts_at_the_clicked_window() {
        let mut g = Gestures::default();
        drag(&mut g, (50, 50), (50, 50), 1_000);
        let probe = Fake { window: Some([10.0, 20.0, 810.0, 620.0]), ..Default::default() };
        assert_eq!(pick(Copied::Image(Some((800, 600))), &g, 1_500, &probe), Some((410.0, 320.0)));
        assert_eq!(pick(Copied::Image(Some((300, 300))), &g, 1_500, &probe), None);
    }

    #[test]
    fn a_copied_image_starts_at_the_right_clicked_picture_or_point() {
        let mut g = Gestures::default();
        g.right_up((700, 800), 1_000);
        let copied = Copied::Image(Some((3000, 2000)));
        let picture = Fake { image: Some([600.0, 700.0, 900.0, 900.0]), ..Default::default() };
        assert_eq!(pick(copied, &g, 2_000, &picture), Some((750.0, 800.0)));
        assert_eq!(pick(copied, &g, 2_000, &Fake::default()), Some((700.0, 800.0)));
    }

    #[test]
    fn a_full_screen_capture_starts_at_the_screen_centre() {
        let g = Gestures::default();
        let probe = Fake { focused: Some((5.0, 5.0)), ..Default::default() };
        assert_eq!(pick(Copied::Image(Some((1920, 1080))), &g, 0, &probe), Some((960.0, 540.0)));
        assert_eq!(pick(Copied::Image(Some((640, 480))), &g, 0, &probe), None);
        assert_eq!(pick(Copied::Image(None), &g, 0, &probe), None);
    }

    #[test]
    fn files_prefer_the_right_click_then_the_selected_items() {
        let mut g = Gestures::default();
        let probe = Fake {
            selection: Some((1.0, 1.0)),
            items: Some((200.0, 300.0)),
            focused: Some((900.0, 900.0)),
            ..Default::default()
        };
        assert_eq!(pick(Copied::Files, &g, 0, &probe), Some((200.0, 300.0)));
        g.right_up((40, 50), 1_000);
        assert_eq!(pick(Copied::Files, &g, 2_000, &probe), Some((40.0, 50.0)));
        let bare = Fake { focused: Some((900.0, 900.0)), ..Default::default() };
        assert_eq!(pick(Copied::Files, &Gestures::default(), 0, &bare), Some((900.0, 900.0)));
    }

    #[test]
    fn text_follows_the_candidate_order() {
        let mut g = Gestures::default();
        drag(&mut g, (100, 100), (300, 140), 0);
        g.right_up((400, 400), 100);
        // Two clicks later the right click no longer counts; add them first.
        let mut probe = Fake {
            selection: Some((10.0, 10.0)),
            caret: Some((20.0, 20.0)),
            focused: Some((30.0, 30.0)),
            ..Default::default()
        };
        assert_eq!(pick(Copied::Other, &g, 200, &probe), Some((10.0, 10.0)));
        probe.selection = None;
        // Right click before drag.
        assert_eq!(pick(Copied::Other, &g, 200, &probe), Some((400.0, 400.0)));
        // A "Copy" button click (after the menu was dismissed) beats the right click.
        drag(&mut g, (5, 5), (5, 5), 300);
        drag(&mut g, (600, 600), (600, 600), 400);
        assert_eq!(pick(Copied::Other, &g, 500, &probe), Some((600.0, 600.0)));
        // Once that click is old, the drag.
        assert_eq!(pick(Copied::Other, &g, 5_000, &probe), Some((200.0, 120.0)));
        // Once the drag is old, the caret, then the focused control.
        assert_eq!(pick(Copied::Other, &g, 30_001, &probe), Some((20.0, 20.0)));
        probe.caret = None;
        assert_eq!(pick(Copied::Other, &g, 30_001, &probe), Some((30.0, 30.0)));
    }

    #[test]
    fn only_visible_rectangles_count() {
        let screen = [0.0, 0.0, 1920.0, 1080.0];
        let rects = [
            [100.0, 100.0, 300.0, 120.0],
            [100.0, 120.0, 500.0, 140.0],
            [100.0, 4000.0, 900.0, 4020.0], // scrolled out of view
            [700.0, 700.0, 700.0, 720.0],   // empty
        ];
        assert_eq!(union_visible(rects, screen), Some([100.0, 100.0, 500.0, 140.0]));
        // Partly off screen: only the visible part counts.
        assert_eq!(union_visible([[-50.0, -10.0, 50.0, 10.0]], screen), Some([0.0, 0.0, 50.0, 10.0]));
        assert_eq!(union_visible([[2000.0, 0.0, 2100.0, 10.0]], screen), None);
    }
}
