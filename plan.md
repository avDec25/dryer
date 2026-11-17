# plan.md — "Dryer": a clothesline clipboard for Windows 11

## Context
Dryer is a Windows 11 clipboard-history app with no conventional window. A thin rope hangs across the full width of the monitor from the top edge, sagging in a curve. Each copied item hangs from it as a pegged card. Only the **last 10** clips are kept. **Clicking a card puts that clip back on the clipboard** as the newest item. The app must look good, run fast, and use as little memory as possible.

The working folder `c:\Users\amarv\dryer` is empty, so this is a greenfield project.

## Technology stack
The app uses only these technologies:

- **Rust** (stable), calling Win32 through Microsoft's official `windows` crate (the only dependency).
- **Win32**: windows, messages, clipboard, tray, hotkey, region, registry, and DWM color APIs.
- **Direct2D**: all drawing (rope, cards, thumbnails, shadows, and effects).
- **DirectComposition**: binds the Direct2D output to the window and composites per-pixel alpha on the GPU.

Direct2D and DirectComposition require a D3D11 device and a DXGI swap chain underneath. These are plumbing only (no separate features), used just for the swap chain Direct2D draws into.

Rejected alternatives (from research): Electron (~150–200 MB idle), Tauri/WebView2 (multiple browser-engine processes), and WinUI 3 / WPF (built around conventional windows and a heavy runtime).

## Window and rendering

- **Window:** one borderless popup with `WS_EX_NOREDIRECTIONBITMAP | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE`. It has no taskbar button, never takes focus, and has no GDI redirection surface.
- **Swap chain:** `CreateSwapChainForComposition`, flip model, premultiplied alpha, 2 buffers, sized to the line band only.
- **Composition:** a DirectComposition visual binds the swap chain to the window. Direct2D draws into it each frame.
- **Size:** one window per primary monitor, spanning the full monitor width (`MonitorFromPoint` / `GetMonitorInfo` work area). Height is only the line band, about 260 px × DPI scale.
- **Click-through:** `SetWindowRgn` with the union of the card rectangles plus a thin rope strip, so clicks elsewhere reach the apps underneath. Recompute at most about 30 Hz while cards swing, using each card's swing bounding box to avoid jitter.
- **DPI and displays:** handle `WM_DPICHANGED` and `WM_DISPLAYCHANGE` by resizing and rebuilding the swap chain.

## Input and controls

- Hover uses `WM_MOUSEMOVE` / `WM_MOUSELEAVE` (`TrackMouseEvent`). The hovered card lifts and swings slightly.
- `WM_LBUTTONUP` on a card restores that clip.
- Right-click opens a native `TrackPopupMenu` with Delete, Pin, Clear all, and Quit.
- Tray icon (`Shell_NotifyIconW`) with Show/Hide and Quit.
- Global hotkey `Win+Shift+C` (`RegisterHotKey`) slides the line up out of view or drops it back down.
- Auto-hide while a fullscreen app is in front, checked with `SHQueryUserNotificationState`.

## Clipboard capture

- Listen with `AddClipboardFormatListener` and `WM_CLIPBOARDUPDATE`. This is event-driven, with no polling.
- Skip changes this app made itself: compare `GetClipboardSequenceNumber()` with the number recorded during restore.
- **Privacy:** skip the clip if `ExcludeClipboardContentFromMonitorProcessing` or `CanIncludeInClipboardHistory` = 0 is present (password managers set these).
- If `OpenClipboard` fails because another app holds it, retry up to 5 times, 15 ms apart, using a `SetTimer` one-shot. Never sleep on the UI thread.
- **Stored formats** (enough for a faithful restore):
  - Text: `CF_UNICODETEXT`, plus registered `HTML Format` and `Rich Text Format` if present.
  - Image: `CF_DIBV5` or `CF_DIB`, stored as raw bytes. Only 24 and 32 bpp DIBs are supported; others are not captured.
  - Files: `CF_HDROP`, stored as a path list.
- **Size cap:** a payload over 8 MB is not captured. This keeps memory predictable without an encoder.
- **Deduplicate:** hash the primary payload with Rust's standard hasher. If the clip is already on the line, move it to the front with an animation instead of adding a duplicate.

## Ring of exactly 10

- A fixed `[Option<Clip>; 10]` array with a head index. No `Vec` growth.
- A new clip evicts the oldest. Its card drops off the line with a short drop-and-fade animation.
- Pinned clips are skipped during eviction, but the ring still never holds more than 10.

## Restoring a clip on click

- `OpenClipboard(hwnd)`, `EmptyClipboard`, `SetClipboardData` for every stored format (`GlobalAlloc(GMEM_MOVEABLE)`), then `CloseClipboard`.
- Record the new sequence number so the listener ignores this change.
- Move the clip to slot 0 (newest), animate it sliding to the front, and give the rope a small "tug" bounce.

## Visuals and animation

- **Rope:** a cubic Bézier from (0, −4) to (W, −4), sagging by `sag(t)`. Drawn as a soft shadow, then a stroke with a twisted-fiber dash pattern (`D2D1_STROKE_STYLE` custom dash plus a subtle linear gradient). Card attachment points are sampled evenly along the arc length.
- **Physics** (`physics.rs`): fixed-step semi-implicit Euler at 120 Hz.
  - The rope sag is a damped spring. A new clip impulses it downward so it bounces and settles.
  - Each card is a damped pendulum driven by the rope motion at its attachment point, so motion ripples outward.
  - Spring constants live in one `const` table.
- **Cards:** rounded rectangles in a paper color with a soft drop shadow (`CLSID_D2D1Shadow` effect, cached per card size). Each has a wooden peg at the top, the content preview, and a type glyph with relative time.
  - Text cards: preview of the first lines. **Open question, see below.**
  - Image cards: a downscaled thumbnail, scaled with the Direct2D Scale effect and cached as a D2D bitmap.
  - File cards: a generic page glyph drawn with Direct2D, plus the file name. **Also see the open question.**
- **Theme:** follow light/dark mode (`ShouldAppsUseDarkMode` registry value, re-read on `WM_SETTINGCHANGE`) and the accent color (`DwmGetColorizationColor`) for the peg and hover tint.
- **Render on demand:** frames are produced only while a spring or animation is active, paced by `IDXGISwapChain2::GetFrameLatencyWaitableObject` inside `MsgWaitForMultipleObjectsEx`. At rest, nothing renders.

## Memory budget (target: under 20 MB private working set when idle)

- Swap chain sized to width × band height (about 2560 × 260 × 4 B × 2 ≈ 5 MB at 1440p).
- Thumbnails are 10 small cached D2D bitmaps. Full-size image payloads are kept only as the capped raw bytes.
- Call `SetProcessWorkingSetSize(-1, -1)` after large captures to trim, and `IDXGIDevice3::Trim()` when hidden.
- Single UI thread, no background workers.

## Robustness

- Single-instance named mutex.
- On `D2DERR_RECREATE_TARGET` or `DXGI_ERROR_DEVICE_REMOVED`, rebuild the devices and re-create thumbnails from the stored clips.
- `RemoveClipboardFormatListener` on exit.
- Optional "Start with Windows" via a `HKCU\...\Run` registry entry, toggled from the tray menu.
- **Persistence: none.** Clips live only in memory, for privacy.

## Open question (decision needed)

Direct2D has no text API of its own. Text on cards, and file names, need **DirectWrite**, which is outside the four listed technologies. Options:
1. Allow DirectWrite for text only (recommended; it is the standard companion to Direct2D).
2. Draw no text at all: text cards show only a shape or color swatch, and file cards show only the glyph.

Until this is decided, the plan leaves text out of the card design.

## Implementation milestones

1. **Scaffold:** `cargo new dryer`, release profile (`opt-level=3`, `lto="fat"`, `codegen-units=1`, `panic="abort"`, `strip=true`), app manifest (PerMonitorV2 DPI), single instance, message loop, tray icon with Quit.
2. **Overlay:** transparent DirectComposition window across the top of the screen with a static rope. Verify transparency, topmost, no focus steal, and click-through.
3. **Clipboard capture and ring:** listener, format reads, privacy flags, dedupe, 10-slot ring.
4. **Cards and thumbnails:** text (per the open question), image, and file cards on the rope; region-based hit testing.
5. **Click to restore:** write-back of all formats, self-change suppression, move to front.
6. **Physics and polish:** rope bounce, pendulum ripple, hover lift, eviction drop, theme and accent, hide/show hotkey, fullscreen auto-hide.
7. **Hardening and profiling:** device-lost handling, DPI and monitor changes, memory and CPU measurements.

## Verification

- `cargo build --release`, then run `target\release\dryer.exe`.
- **Functional:**
  - Copy text in Notepad, an image (Win+Shift+S snip), and files in Explorer. Each appears as a card with the rope bounce.
  - Copy 12 items and confirm only 10 cards remain, with the oldest dropping off.
  - Click card #5, paste into Notepad or Paint, and confirm the content matches and the card moves to the front without a duplicate.
  - Copy a password from a password manager and confirm it is not captured.
  - Click empty areas of the band and confirm the click reaches the window underneath.
- **Performance:**
  - `Get-Process dryer | Select-Object WS, PM`: idle private memory under 20 MB.
  - CPU and GPU near 0% in Task Manager when settled.
  - Smooth animation during the bounce.
- **Resilience:** change display scaling, unplug or replug a monitor, switch light/dark theme, and lock/unlock the session. The app keeps working.

## Sources
- [winpane – DirectComposition overlay design (Rust)](https://peteretelej.github.io/winpane/design)
- [riley-x/TransparentWindow – Direct2D + DirectComposition per-pixel alpha](https://www.github.com/riley-x/TransparentWindow)
- [MSDN Magazine – DirectComposition high-performance window layering](https://msdn.microsoft.com/en-us/magazine/dn745861.aspx)
- [AddClipboardFormatListener (Microsoft Learn)](https://learn.microsoft.com/en-us/windows/win32/api/Winuser/nf-winuser-addclipboardformatlistener)
- [WM_CLIPBOARDUPDATE (Microsoft Learn)](https://learn.microsoft.com/fi-fi/windows/win32/dataxchg/wm-clipboardupdate)
- [The Old New Thing – clipboard format listener vs. viewer chain](https://devblogs.microsoft.com/oldnewthing/20110919-00/?p=9613/)
- [clipboard-win monitor source (reference impl)](https://docs.rs/clipboard-win/latest/src/clipboard_win/monitor.rs.html)
- [Win32 vs Electron/Tauri memory comparison](https://dev.to/gerus_team/your-electron-app-is-bloatware-we-tested-win32-and-got-a-27x-memory-reduction-8nl)
