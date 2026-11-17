//! Dryer: a clothesline clipboard history for Windows 11.
//! A borderless, click-through strip across the top of the primary monitor.

#![windows_subsystem = "windows"]

mod card;
mod clipboard;
mod physics;
mod render;
mod ring;

use std::time::{Instant, SystemTime, UNIX_EPOCH};

use windows::Win32::Foundation::{
    ERROR_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTOPRIMARY, MONITORINFO, MonitorFromPoint, ScreenToClient,
    ValidateRect,
};
use windows::Win32::System::DataExchange::{
    AddClipboardFormatListener, GetClipboardSequenceNumber, RemoveClipboardFormatListener,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForSystem, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    MOD_SHIFT, MOD_WIN, RegisterHotKey, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent,
    UnregisterHotKey,
};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::w;

use crate::card::CARD_W;
use crate::clipboard::{Capture, Clip};
use crate::physics::{Breeze, Rope};
use crate::render::{PEG_DROP, ROPE_Y};
use crate::ring::Ring;

/// Tall enough for the tallest card hanging from a sagging rope.
const BAND_DIP: f32 = 300.0;
const WM_TRAY: u32 = WM_APP + 1;
const TIMER_FRAME: usize = 1;
const TIMER_RETRY: usize = 2;
const TIMER_BREEZE: usize = 3;
const FRAME_MS: u32 = 16;
/// Frame interval for gentle motion (breeze) once nothing has happened for a while.
const CALM_FRAME_MS: u32 = 33;
const CALM_AFTER_S: f32 = 3.0;
const FIRST_BREEZE_MS: u32 = 4_000;
const HOTKEY_ID: i32 = 1;
const TRAY_ID: u32 = 1;
const CMD_TOGGLE: usize = 1;
const CMD_QUIT: usize = 2;
const RETRY_MS: u32 = 15;
const MAX_RETRIES: u32 = 5;
/// Downward kick on the rope where a clicked card is unclipped.
const RESTORE_KICK: f32 = 90.0;
/// Push given to a card when the pointer brushes it (rad/s).
const HOVER_NUDGE: f32 = 0.9;
/// A new card starts this much larger and shrinks onto the line.
const GROW_START: f32 = 0.25;
/// A new card always flies at least this far up to the line.
const MIN_FLIGHT: f32 = 40.0;

/// Breeze phase for a card: differs per card, and travels along the line.
fn wind_phase(hash: u64, cx: f32) -> f32 {
    (hash % 6283) as f32 / 1000.0 - cx * 0.004
}

/// How far below its clip a card's centre hangs (the pendulum length).
fn hang_len(card_h: f32) -> f32 {
    PEG_DROP + card_h * 0.55
}

struct App {
    hwnd: HWND,
    ring: Ring,
    engine: card::Engine,
    /// Clips pushed out of the ring, sliding off the left edge (absolute x in `hx`).
    departing: Vec<Clip>,
    scene: Option<render::Scene>,
    rope: Rope,
    breeze: Breeze,
    visible: bool,
    hover: Option<usize>,
    tracking_mouse: bool,
    /// Sequence number produced by our own clipboard write; changes with it are ignored.
    own_seq: u32,
    retries: u32,
    last_tick: Instant,
    timer_on: bool,
    frame_ms: u32,
    /// Last copy, click, hover or show; after a quiet spell frames slow down.
    last_event: Instant,
    dpi: u32,
    px_w: i32,
    px_h: i32,
    width_dip: f32,
}

impl App {
    fn new(hwnd: HWND, engine: card::Engine) -> Self {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(1);
        Self {
            hwnd,
            ring: Ring::new(),
            engine,
            departing: Vec::new(),
            scene: None,
            rope: Rope::new(),
            breeze: Breeze::new(seed),
            visible: false,
            hover: None,
            tracking_mouse: false,
            own_seq: 0,
            retries: 0,
            last_tick: Instant::now(),
            timer_on: false,
            frame_ms: FRAME_MS,
            last_event: Instant::now(),
            dpi: 96,
            px_w: 0,
            px_h: 0,
            width_dip: 0.0,
        }
    }

    fn scale(&self) -> f32 {
        self.dpi as f32 / 96.0
    }

    /// Convert a window-client pixel position to DIPs.
    fn to_dip(&self, x: i32, y: i32) -> (f32, f32) {
        let s = self.scale();
        (x as f32 / s, y as f32 / s)
    }

    fn hit(&self, x: f32, y: f32) -> Option<usize> {
        render::hit(&self.ring, &self.rope, self.width_dip, x, y)
    }

    /// The pointer position in window DIPs.
    fn cursor_dip(&self) -> (f32, f32) {
        let mut pt = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut pt);
            let _ = ScreenToClient(self.hwnd, &mut pt);
        }
        self.to_dip(pt.x, pt.y)
    }

    /// Current on-screen x of every card on the line, by content hash.
    fn positions(&self) -> Vec<(u64, f32)> {
        let n = self.ring.len();
        (0..n)
            .filter_map(|i| self.ring.get(i).map(|c| (c.hash, render::ring_cx(c, i, n, self.width_dip))))
            .collect()
    }

    /// Place the strip across the top of the primary monitor at its current DPI.
    fn reposition(&mut self) {
        unsafe {
            let mon = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
            let mut info = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            let _ = GetMonitorInfoW(mon, &mut info);
            let r: RECT = info.rcMonitor;

            self.dpi = GetDpiForSystem().max(96);
            let s = self.scale();
            self.px_w = r.right - r.left;
            self.px_h = (BAND_DIP * s).round() as i32;
            self.width_dip = self.px_w as f32 / s;

            // The swap chain must be recreated at the new size.
            self.drop_gpu();

            let _ = SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                r.left,
                r.top,
                self.px_w,
                self.px_h,
                SWP_NOACTIVATE,
            );
        }
        self.kick();
    }

    fn drop_gpu(&mut self) {
        self.scene = None;
        self.ring.clear_gpu_cache();
        for c in self.departing.iter_mut() {
            c.thumb = None;
            c.sprite = None;
        }
    }

    fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
        unsafe {
            if visible {
                let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
                self.last_tick = Instant::now();
                self.last_event = Instant::now();
                self.kick();
            } else {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
                self.stop_timer();
            }
        }
    }

    /// Start the frame timer (if needed) and draw a frame now.
    fn kick(&mut self) {
        if !self.visible {
            return;
        }
        if !self.timer_on {
            self.last_tick = Instant::now();
            self.frame_ms = self.frame_interval();
            unsafe { SetTimer(Some(self.hwnd), TIMER_FRAME, self.frame_ms, None) };
            self.timer_on = true;
        }
        self.render();
    }

    /// 60 fps right after something happens, 30 fps for calm motion like the breeze.
    fn frame_interval(&self) -> u32 {
        if self.last_event.elapsed().as_secs_f32() > CALM_AFTER_S {
            CALM_FRAME_MS
        } else {
            FRAME_MS
        }
    }

    fn stop_timer(&mut self) {
        if self.timer_on {
            unsafe {
                let _ = KillTimer(Some(self.hwnd), TIMER_FRAME);
            }
            self.timer_on = false;
        }
    }

    fn render(&mut self) {
        if !self.visible {
            return;
        }
        if self.scene.is_none() {
            match render::Scene::new(self.hwnd, self.dpi, self.px_w, self.px_h) {
                Ok(s) => self.scene = Some(s),
                Err(_) => return,
            }
        }
        if let Some(scene) = self.scene.as_ref() {
            let drawn = scene.draw(&mut self.ring, &mut self.departing, &self.rope, self.width_dip);
            if drawn.is_err() {
                // Device lost or target invalid: rebuild on the next frame.
                self.drop_gpu();
            }
        }
    }

    /// Advance physics by the elapsed time, then draw. Stops the timer when at rest.
    fn on_frame(&mut self) {
        let now = Instant::now();
        let mut dt = (now - self.last_tick).as_secs_f32().min(0.05);
        self.last_tick = now;
        let w = self.width_dip;
        let n = self.ring.len();

        // Each card weighs the rope down where it hangs (not while still flying in).
        self.rope.clear_loads();
        for (i, c) in self.ring.iter_mut().enumerate() {
            let cx = card::slot_x(i, n, w) + c.motion.hx.x;
            let attached = (1.0 - c.motion.hy.x / MIN_FLIGHT).clamp(0.0, 1.0);
            self.rope.add_load(cx / w, physics::card_load(c.card_h) * attached);
        }
        for c in self.departing.iter() {
            self.rope.add_load(c.motion.hx.x / w, physics::card_load(c.card_h));
        }

        while dt > 0.0 {
            let h = dt.min(physics::SUBSTEP);
            self.rope.step(h);
            self.breeze.step(h);
            for (i, c) in self.ring.iter_mut().enumerate() {
                let cx = card::slot_x(i, n, w) + c.motion.hx.x;
                let wind = self.breeze.force(wind_phase(c.hash, cx));
                c.motion.step(h, hang_len(c.card_h), wind);
            }
            for c in self.departing.iter_mut() {
                let wind = self.breeze.force(wind_phase(c.hash, c.motion.hx.x));
                c.motion.step(h, hang_len(c.card_h), wind);
            }
            dt -= h;
        }

        // Departing clips are dropped once they have slid off the left edge.
        self.departing.retain(|c| c.motion.hx.x > -CARD_W);

        let active = self.rope.active()
            || self.breeze.active()
            || !self.departing.is_empty()
            || self.ring.iter_mut().any(|c| c.motion.active());
        if !active {
            for c in self.ring.iter_mut() {
                c.motion.settle();
            }
            self.render();
            self.stop_timer();
        } else {
            let want = self.frame_interval();
            if want != self.frame_ms {
                self.frame_ms = want;
                unsafe { SetTimer(Some(self.hwnd), TIMER_FRAME, want, None) };
            }
            self.render();
        }
    }

    fn on_clipboard_update(&mut self) {
        let seq = unsafe { GetClipboardSequenceNumber() };
        if seq != 0 && seq == self.own_seq {
            return;
        }
        self.retries = MAX_RETRIES;
        self.try_capture();
    }

    fn try_capture(&mut self) {
        match clipboard::capture(self.hwnd) {
            Capture::Busy => {
                if self.retries > 0 {
                    self.retries -= 1;
                    unsafe { SetTimer(Some(self.hwnd), TIMER_RETRY, RETRY_MS, None) };
                }
            }
            Capture::Skip => {}
            Capture::Got(mut clip) => {
                // Lay out the text and fix the card height once, up front.
                let _ = self.engine.prepare(&mut clip);
                let before = self.positions();
                let evicted = self.ring.insert(clip);
                let from = self.cursor_dip();
                self.reflow(&before, evicted, Some(from));
                self.kick();
            }
        }
    }

    /// Put clip `idx` back on the clipboard as the newest item.
    fn restore(&mut self, idx: usize) {
        let seq = match self.ring.get(idx) {
            Some(clip) => clipboard::write(self.hwnd, &clip.formats),
            None => return,
        };
        if let Some(s) = seq {
            self.own_seq = s;
        }
        let before = self.positions();
        if let Some(&(_, x)) = before.get(idx) {
            self.rope.kick(x / self.width_dip, RESTORE_KICK);
        }
        self.ring.move_to_front(idx);
        self.reflow(&before, None, None);
        self.kick();
    }

    /// After the line changes, every card keeps its on-screen position and then
    /// moves to its new slot. A new card flies up from `arrival` (the pointer);
    /// an evicted card slides off the left edge.
    fn reflow(&mut self, before: &[(u64, f32)], evicted: Option<Clip>, arrival: Option<(f32, f32)>) {
        self.hover = None;
        self.mark_event();
        let w = self.width_dip;
        let n = self.ring.len();
        for i in 0..n {
            let Some(c) = self.ring.get_mut(i) else { continue };
            let base = card::slot_x(i, n, w);
            match before.iter().find(|(h, _)| *h == c.hash) {
                Some(&(_, old_x)) => c.motion.shift(old_x - (base + c.motion.hx.x)),
                None => {
                    let (ax, ay) = arrival.unwrap_or((base + CARD_W, 0.0));
                    let rest_top = ROPE_Y + PEG_DROP;
                    let dy = (ay - rest_top).clamp(MIN_FLIGHT, BAND_DIP - rest_top);
                    c.motion.launch(ax.clamp(0.0, w) - base, dy, GROW_START);
                }
            }
        }
        if let Some(mut c) = evicted {
            let old_x = before
                .iter()
                .find(|(h, _)| *h == c.hash)
                .map_or(0.0, |p| p.1);
            c.motion.rebase(old_x);
            c.motion.hx_target = -1.5 * CARD_W;
            self.departing.push(c);
        }
    }

    /// The pointer brushed a card: push it away from the side the pointer came from.
    fn set_hover(&mut self, h: Option<usize>, x: f32) {
        if self.hover == h {
            return;
        }
        self.hover = h;
        let Some(i) = h else { return };
        let (n, w) = (self.ring.len(), self.width_dip);
        if let Some(c) = self.ring.get_mut(i) {
            let cx = card::slot_x(i, n, w) + c.motion.hx.x;
            c.motion.nudge((x - cx).signum() * HOVER_NUDGE);
        }
        self.mark_event();
        self.kick();
    }

    /// Something happened: animate at full frame rate for a while.
    fn mark_event(&mut self) {
        self.last_event = Instant::now();
        if self.timer_on && self.frame_ms != FRAME_MS {
            self.frame_ms = FRAME_MS;
            unsafe { SetTimer(Some(self.hwnd), TIMER_FRAME, FRAME_MS, None) };
        }
    }

    fn on_breeze(&mut self) {
        if self.visible && self.ring.len() > 0 {
            self.breeze.start();
            self.kick();
        }
        let next = self.breeze.next_delay_ms();
        unsafe { SetTimer(Some(self.hwnd), TIMER_BREEZE, next, None) };
    }

    fn toggle(&mut self) {
        let next = !self.visible;
        self.set_visible(next);
    }

    fn on_mouse_move(&mut self, lp: LPARAM) {
        if !self.tracking_mouse {
            let mut tme = TRACKMOUSEEVENT {
                cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: self.hwnd,
                dwHoverTime: 0,
            };
            unsafe {
                let _ = TrackMouseEvent(&mut tme);
            }
            self.tracking_mouse = true;
        }
        let (x, y) = self.to_dip(lparam_x(lp), lparam_y(lp));
        let h = self.hit(x, y);
        self.set_hover(h, x);
    }

    fn on_mouse_leave(&mut self) {
        self.tracking_mouse = false;
        self.set_hover(None, 0.0);
    }

    /// Cards catch the mouse; everything else passes clicks through to the window below.
    fn on_nc_hit_test(&mut self, lp: LPARAM) -> LRESULT {
        let mut pt = POINT { x: lparam_x(lp), y: lparam_y(lp) };
        unsafe {
            let _ = ScreenToClient(self.hwnd, &mut pt);
        }
        let (x, y) = self.to_dip(pt.x, pt.y);
        if self.hit(x, y).is_some() {
            LRESULT(HTCLIENT as isize)
        } else {
            self.set_hover(None, x);
            LRESULT(HTTRANSPARENT as isize)
        }
    }

    fn add_tray(&self) {
        let mut nid = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: TRAY_ID,
            uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
            uCallbackMessage: WM_TRAY,
            hIcon: unsafe { LoadIconW(None, IDI_APPLICATION).unwrap_or_default() },
            ..Default::default()
        };
        let tip: Vec<u16> = "Dryer".encode_utf16().collect();
        nid.szTip[..tip.len()].copy_from_slice(&tip);
        unsafe {
            let _ = Shell_NotifyIconW(NIM_ADD, &nid);
        }
    }

    fn remove_tray(&self) {
        let nid = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: TRAY_ID,
            ..Default::default()
        };
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
        }
    }

    fn show_tray_menu(&self) {
        unsafe {
            let Ok(menu) = CreatePopupMenu() else { return };
            let _ = AppendMenuW(menu, MF_STRING, CMD_TOGGLE, w!("Show / Hide"));
            let _ = AppendMenuW(menu, MF_STRING, CMD_QUIT, w!("Quit"));
            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            // Required so the menu dismisses when the user clicks elsewhere.
            let _ = SetForegroundWindow(self.hwnd);
            let _ = TrackPopupMenu(menu, TPM_RIGHTBUTTON, pt.x, pt.y, Some(0), self.hwnd, None);
            let _ = DestroyMenu(menu);
        }
    }
}

fn lparam_x(lp: LPARAM) -> i32 {
    (lp.0 & 0xFFFF) as i16 as i32
}

fn lparam_y(lp: LPARAM) -> i32 {
    ((lp.0 >> 16) & 0xFFFF) as i16 as i32
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        let p = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut App;
        if p.is_null() {
            return DefWindowProcW(hwnd, msg, wp, lp);
        }
        let app = &mut *p;

        match msg {
            WM_NCHITTEST => return app.on_nc_hit_test(lp),
            WM_MOUSEMOVE => app.on_mouse_move(lp),
            WM_MOUSELEAVE => app.on_mouse_leave(),
            WM_LBUTTONUP => {
                let (x, y) = app.to_dip(lparam_x(lp), lparam_y(lp));
                if let Some(i) = app.hit(x, y) {
                    app.restore(i);
                }
            }
            WM_CLIPBOARDUPDATE => app.on_clipboard_update(),
            WM_TIMER => match wp.0 {
                TIMER_FRAME => app.on_frame(),
                TIMER_RETRY => {
                    let _ = KillTimer(Some(hwnd), TIMER_RETRY);
                    app.try_capture();
                }
                TIMER_BREEZE => app.on_breeze(),
                _ => {}
            },
            WM_HOTKEY => {
                if wp.0 as i32 == HOTKEY_ID {
                    app.toggle();
                }
            }
            WM_TRAY => match (lp.0 as u32) & 0xFFFF {
                WM_LBUTTONUP => app.toggle(),
                WM_RBUTTONUP => app.show_tray_menu(),
                _ => {}
            },
            WM_COMMAND => match wp.0 & 0xFFFF {
                CMD_TOGGLE => app.toggle(),
                CMD_QUIT => {
                    let _ = DestroyWindow(hwnd);
                }
                _ => {}
            },
            WM_PAINT => {
                let _ = ValidateRect(Some(hwnd), None);
                app.render();
            }
            WM_DISPLAYCHANGE | WM_DPICHANGED => app.reposition(),
            WM_DESTROY => {
                let _ = RemoveClipboardFormatListener(hwnd);
                let _ = UnregisterHotKey(Some(hwnd), HOTKEY_ID);
                app.remove_tray();
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                drop(Box::from_raw(p));
                PostQuitMessage(0);
                return LRESULT(0);
            }
            _ => {}
        }
        DefWindowProcW(hwnd, msg, wp, lp)
    }
}

fn main() -> windows::core::Result<()> {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);

        // Only one instance may run; a second launch exits immediately.
        let _mutex = CreateMutexW(None, true, w!("Local\\DryerSingleInstance"))?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return Ok(());
        }

        let hinst = GetModuleHandleW(None)?;
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst.into(),
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            lpszClassName: w!("DryerOverlay"),
            ..Default::default()
        };
        RegisterClassExW(&class);

        let hwnd = CreateWindowExW(
            WS_EX_NOREDIRECTIONBITMAP | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            w!("DryerOverlay"),
            w!("Dryer"),
            WS_POPUP,
            0,
            0,
            1,
            1,
            None,
            None,
            Some(hinst.into()),
            None,
        )?;

        let engine = card::Engine::new().expect("DirectWrite is unavailable");
        let app = Box::into_raw(Box::new(App::new(hwnd, engine)));
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, app as isize);

        let app = &mut *app;
        app.add_tray();
        let _ = AddClipboardFormatListener(hwnd);
        let _ = RegisterHotKey(Some(hwnd), HOTKEY_ID, MOD_WIN | MOD_SHIFT, 'C' as u32);
        app.reposition();
        // The rope starts taut and drops into its sag when the line appears.
        app.set_visible(true);
        SetTimer(Some(hwnd), TIMER_BREEZE, FIRST_BREEZE_MS, None);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    Ok(())
}
