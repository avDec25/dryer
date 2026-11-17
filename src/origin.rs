//! Finds where on screen the copied content came from, so a new card can fly in
//! from that spot.

use std::ffi::c_void;
use std::ptr::null_mut;

use windows::Win32::Foundation::POINT;
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, SAFEARRAY,
};
use windows::Win32::System::Ole::{
    SafeArrayAccessData, SafeArrayDestroy, SafeArrayGetLBound, SafeArrayGetUBound,
    SafeArrayUnaccessData,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomation6, IUIAutomationElement, IUIAutomationTextPattern,
    UIA_TextPatternId,
};
use windows::Win32::UI::WindowsAndMessaging::{GUITHREADINFO, GetGUIThreadInfo};
use windows::core::{IUnknown, Interface};

/// How long to wait on a slow or hung application before giving up (ms).
const UIA_TIMEOUT_MS: u32 = 150;

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

    /// Screen position (physical pixels) of the copied content, best guess first:
    /// the selected text, then the text caret, then the focused control.
    /// None if nothing could be found.
    pub fn locate(&self) -> Option<(f32, f32)> {
        unsafe {
            let focused = self.uia.as_ref().and_then(|u| u.GetFocusedElement().ok());
            if let Some(el) = focused.as_ref() {
                if let Some(p) = selection_center(el) {
                    return Some(p);
                }
            }
            if let Some(p) = caret_center() {
                return Some(p);
            }
            focused.as_ref().and_then(|el| element_center(el))
        }
    }
}

/// Centre of the first line of the focused element's selected text.
unsafe fn selection_center(el: &IUIAutomationElement) -> Option<(f32, f32)> {
    unsafe {
        let pattern: IUIAutomationTextPattern = el.GetCurrentPatternAs(UIA_TextPatternId).ok()?;
        let selection = pattern.GetSelection().ok()?;
        if selection.Length().ok()? < 1 {
            return None;
        }
        let range = selection.GetElement(0).ok()?;
        let rects = range.GetBoundingRectangles().ok()?;
        let [l, t, w, h] = first_rect(rects)?;
        if w <= 0.0 || h <= 0.0 {
            return None;
        }
        Some(((l + w / 2.0) as f32, (t + h / 2.0) as f32))
    }
}

/// The first rectangle (left, top, width, height) in a UI Automation rectangle array.
/// Takes ownership of, and frees, the array.
unsafe fn first_rect(sa: *mut SAFEARRAY) -> Option<[f64; 4]> {
    unsafe {
        if sa.is_null() {
            return None;
        }
        let mut out = None;
        let lower = SafeArrayGetLBound(sa, 1).ok()?;
        let upper = SafeArrayGetUBound(sa, 1).ok()?;
        if upper - lower + 1 >= 4 {
            let mut data: *mut c_void = null_mut();
            if SafeArrayAccessData(sa, &mut data).is_ok() {
                let d = data as *const f64;
                out = Some([*d, *d.add(1), *d.add(2), *d.add(3)]);
                let _ = SafeArrayUnaccessData(sa);
            }
        }
        let _ = SafeArrayDestroy(sa);
        out
    }
}

/// Centre of the system text caret of the foreground window, for classic controls.
unsafe fn caret_center() -> Option<(f32, f32)> {
    unsafe {
        let mut info = GUITHREADINFO {
            cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
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

/// Centre of the focused control, when nothing more precise is known.
unsafe fn element_center(el: &IUIAutomationElement) -> Option<(f32, f32)> {
    unsafe {
        let r = el.CurrentBoundingRectangle().ok()?;
        if r.right <= r.left || r.bottom <= r.top {
            return None;
        }
        Some(((r.left + r.right) as f32 / 2.0, (r.top + r.bottom) as f32 / 2.0))
    }
}
