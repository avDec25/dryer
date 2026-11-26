//! Remembers recent mouse gestures (the last drag, left click and right click), so
//! the source of a copy can be placed where the user pointed. Fed by Raw Input,
//! which only observes: it cannot delay the mouse. Positions are screen pixels,
//! kept in memory only.

use std::ffi::c_void;
use std::mem::size_of;

use windows::Win32::Foundation::{HWND, LPARAM, POINT};
use windows::Win32::UI::Input::{
    GetRawInputData, HRAWINPUT, RAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER, RID_INPUT,
    RIDEV_INPUTSINK, RIM_TYPEMOUSE, RegisterRawInputDevices,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetSystemMetrics, RI_MOUSE_LEFT_BUTTON_DOWN, RI_MOUSE_LEFT_BUTTON_UP,
    RI_MOUSE_RIGHT_BUTTON_DOWN, RI_MOUSE_RIGHT_BUTTON_UP, SM_SWAPBUTTON,
};

/// A screen position in physical pixels.
pub type Point = (i32, i32);

/// The pointer must travel this far with the button down to count as a drag (px).
const DRAG_MIN_PX: i32 = 6;
/// How long a right click can still be what led to a copy (ms).
const RIGHT_CLICK_MS: u64 = 15_000;

#[derive(Clone, Copy)]
struct Click {
    at: Point,
    time: u64,
}

/// Times are milliseconds from any fixed origin.
#[derive(Default)]
pub struct Gestures {
    /// Where the left button went down, while it is held.
    pressed: Option<Point>,
    drag: Option<(Point, Point, u64)>,
    left: Option<Click>,
    right: Option<Click>,
    /// Left clicks and drags since the right click.
    lefts_since_right: u32,
}

impl Gestures {
    pub fn left_down(&mut self, p: Point) {
        self.pressed = Some(p);
    }

    pub fn left_up(&mut self, p: Point, now: u64) {
        let Some(from) = self.pressed.take() else { return };
        // The first left click after a right click picks an item in its menu: it
        // is part of that right click, not a click on something to copy.
        let menu_pick = self.lefts_since_right == 0 && self.right_click(now).is_some();
        self.lefts_since_right = self.lefts_since_right.saturating_add(1);
        if (p.0 - from.0).abs() >= DRAG_MIN_PX || (p.1 - from.1).abs() >= DRAG_MIN_PX {
            self.drag = Some((from, p, now));
        } else if !menu_pick {
            self.left = Some(Click { at: p, time: now });
        }
    }

    pub fn right_up(&mut self, p: Point, now: u64) {
        self.right = Some(Click { at: p, time: now });
        self.lefts_since_right = 0;
    }

    /// The ends of the last drag, if it finished within `max_ms`.
    pub fn drag(&self, now: u64, max_ms: u64) -> Option<(Point, Point)> {
        self.drag
            .filter(|&(_, _, t)| now.saturating_sub(t) <= max_ms)
            .map(|(from, to, _)| (from, to))
    }

    /// The last plain left click, if within `max_ms`.
    pub fn left_click(&self, now: u64, max_ms: u64) -> Option<Point> {
        self.left.filter(|c| now.saturating_sub(c.time) <= max_ms).map(|c| c.at)
    }

    /// The last right click, while it can still explain a copy: recent, and followed
    /// by no more than the click that picked from its menu.
    pub fn right_click(&self, now: u64) -> Option<Point> {
        self.right
            .filter(|c| now.saturating_sub(c.time) <= RIGHT_CLICK_MS && self.lefts_since_right <= 1)
            .map(|c| c.at)
    }

    /// Apply the button transitions of one raw mouse packet. Raw Input reports
    /// physical buttons, so left-handed (`swapped`) settings are applied here.
    fn buttons(&mut self, flags: u16, swapped: bool, p: Point, now: u64) {
        let has = |flag: u32| u32::from(flags) & flag != 0;
        let left = (RI_MOUSE_LEFT_BUTTON_DOWN, RI_MOUSE_LEFT_BUTTON_UP);
        let right = (RI_MOUSE_RIGHT_BUTTON_DOWN, RI_MOUSE_RIGHT_BUTTON_UP);
        let (primary, secondary) = if swapped { (right, left) } else { (left, right) };
        if has(primary.0) {
            self.left_down(p);
        }
        if has(primary.1) {
            self.left_up(p, now);
        }
        if has(secondary.1) {
            self.right_up(p, now);
        }
    }

    /// Handle a `WM_INPUT` message.
    pub fn on_input(&mut self, lp: LPARAM, now: u64) {
        let mut raw = RAWINPUT::default();
        let mut size = size_of::<RAWINPUT>() as u32;
        let flags = unsafe {
            let read = GetRawInputData(
                HRAWINPUT(lp.0 as *mut c_void),
                RID_INPUT,
                Some(&mut raw as *mut RAWINPUT as *mut c_void),
                &mut size,
                size_of::<RAWINPUTHEADER>() as u32,
            );
            if read == u32::MAX || raw.header.dwType != RIM_TYPEMOUSE.0 {
                return;
            }
            raw.data.mouse.Anonymous.Anonymous.usButtonFlags
        };
        // Plain movement, which is most packets, has no button flags.
        if flags == 0 {
            return;
        }
        let mut pt = POINT::default();
        let swapped = unsafe {
            if GetCursorPos(&mut pt).is_err() {
                return;
            }
            GetSystemMetrics(SM_SWAPBUTTON) != 0
        };
        self.buttons(flags, swapped, (pt.x, pt.y), now);
    }
}

/// Ask for mouse input to be delivered to `hwnd` even while other apps are in use.
pub fn register(hwnd: HWND) {
    let mouse = RAWINPUTDEVICE {
        usUsagePage: 0x01, // generic desktop
        usUsage: 0x02,     // mouse
        dwFlags: RIDEV_INPUTSINK,
        hwndTarget: hwnd,
    };
    unsafe {
        let _ = RegisterRawInputDevices(&[mouse], size_of::<RAWINPUTDEVICE>() as u32);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn movement_tells_a_drag_from_a_click() {
        let mut g = Gestures::default();
        g.left_down((100, 100));
        g.left_up((105, 100), 1_000);
        assert_eq!(g.left_click(1_000, 1_500), Some((105, 100)));
        assert_eq!(g.drag(1_000, 5_000), None);

        g.left_down((100, 100));
        g.left_up((106, 100), 2_000);
        assert_eq!(g.drag(2_000, 5_000), Some(((100, 100), (106, 100))));
    }

    #[test]
    fn the_menu_pick_is_not_a_left_click() {
        let mut g = Gestures::default();
        g.right_up((300, 300), 1_000);
        g.left_down((340, 380));
        g.left_up((340, 380), 2_000);
        assert_eq!(g.right_click(2_000), Some((300, 300)));
        assert_eq!(g.left_click(2_000, 1_500), None);
    }

    #[test]
    fn a_second_left_click_voids_the_right_click() {
        let mut g = Gestures::default();
        g.right_up((300, 300), 1_000);
        for t in [2_000, 3_000] {
            g.left_down((50, 60));
            g.left_up((50, 60), t);
        }
        assert_eq!(g.right_click(3_000), None);
        assert_eq!(g.left_click(3_000, 1_500), Some((50, 60)));
    }

    #[test]
    fn gestures_expire() {
        let mut g = Gestures::default();
        g.right_up((1, 2), 0);
        assert_eq!(g.right_click(15_000), Some((1, 2)));
        assert_eq!(g.right_click(15_001), None);

        let mut g = Gestures::default();
        g.left_down((0, 0));
        g.left_up((50, 50), 0);
        assert!(g.drag(5_000, 5_000).is_some());
        assert!(g.drag(5_001, 5_000).is_none());
        g.left_down((9, 9));
        g.left_up((9, 9), 10_000);
        assert!(g.left_click(11_500, 1_500).is_some());
        assert!(g.left_click(11_501, 1_500).is_none());
    }

    #[test]
    fn a_release_without_a_press_is_ignored() {
        let mut g = Gestures::default();
        g.left_up((50, 50), 0);
        assert_eq!(g.left_click(0, 1_500), None);
        assert_eq!(g.drag(0, 5_000), None);
    }

    #[test]
    fn swapped_buttons_follow_the_primary_button() {
        let physical_right = RI_MOUSE_RIGHT_BUTTON_DOWN | RI_MOUSE_RIGHT_BUTTON_UP;
        let mut g = Gestures::default();
        g.buttons(physical_right as u16, true, (7, 8), 0);
        assert_eq!(g.left_click(0, 1_500), Some((7, 8)));
        assert_eq!(g.right_click(0), None);

        let mut g = Gestures::default();
        g.buttons(physical_right as u16, false, (7, 8), 0);
        assert_eq!(g.right_click(0), Some((7, 8)));
    }
}
