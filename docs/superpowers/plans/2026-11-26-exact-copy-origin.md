# Exact Copy Origin Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Every new card flies out of the exact on-screen spot it was copied from.

**Architecture:** `gesture.rs` remembers recent mouse gestures from Raw Input. `origin.rs` picks the source per content type through a pure `choose` function, with OS queries behind a `Probe` trait. The flight is defined in screen space, and Dryer draws the line's slide itself.

**Tech Stack:** Rust 2024, `windows` 0.62 (adds `Win32_UI_Input`, `Win32_Graphics_Dwm`).

**Spec:** `docs/superpowers/specs/2026-11-26-exact-copy-origin-design.md`

## Global Constraints

- Image drag/click window: 5 s; size tolerance 4 px. Right click: 15 s, at most one left click after it. Left click: 1.5 s. Text drag: 30 s.
- Gesture positions stay in memory. No keyboard hooks, low-level hooks or screen pixel reading.
- UI Automation calls keep the 150 ms timeout.
- No commits unless the user asks.

## Review Focus

- Swapped mouse buttons: the primary button is still treated as "left".
- A drag made up and to the left matches the same as one down and to the right.
- Selection rectangles that are off-screen or empty are ignored.
- A copied image *file* starts from the file, not from image-matching rules.
- A source at a screen corner or off the primary monitor is clamped, not lost.

---

### Task 1: Flight in screen space (`physics.rs`, `render.rs`, `main.rs`)

**Produces:** `Motion::launch(dx, rise, grow)`, `Motion::away() -> f32` (1 at launch, 0 landed), `Motion::height() -> f32` (`rise * away`), `Motion::drop_y(slide) -> f32` (`rise*away + slide*(1-away)`), `render::rise_to(source_y, rope_y, card_h, grow) -> f32`, `Scene::draw(.., slide)`, `render::hit(.., slide, x, y)`.

- [ ] Tests: `launch_is_independent_of_the_slide` (`drop_y(-330.0) == rise` right after launch, also for negative rise), `landed_card_follows_the_slide` (after `settle`, `drop_y(-330.0) == -330.0`), `card_centre_starts_on_the_source` (`rope + rise + (PEG_DROP + h/2)*(1+g) == source_y`).
- [ ] Run `cargo test`: fails to compile (missing items).
- [ ] Implement; replace `hy`; remove the compositor offset (`set_offset`); drop `MIN_FLIGHT` clamp (keep it only as the rope-load distance); a re-copied clip (slot 0 with an arrival) launches.
- [ ] Run `cargo test`: passes.

### Task 2: Gesture tracking (`gesture.rs`)

**Produces:** `Gestures::{left_down(p), left_up(p, now), right_up(p, now), drag(now, max_ms) -> Option<(Point, Point)>, left_click(now, max_ms) -> Option<Point>, right_click(now) -> Option<Point>}` with `Point = (i32, i32)`, times in ms; `register(hwnd)`, `Gestures::on_input(lparam, now)`.

- [ ] Tests: movement of 6 px or more is a drag, less is a click; the left click right after a right click is the menu pick (right stays valid, no left click recorded); a second left click voids the right click; each gesture expires at its limit.
- [ ] Run: fails. Implement (swap buttons per `SM_SWAPBUTTON`). Run: passes.

### Task 3: Source selection (`origin.rs`, `clipboard.rs`, `main.rs`)

**Consumes:** Task 2. **Produces:** `clipboard::Copied { Image(Option<(u32,u32)>), Files, Other }`, `Clip::copied()`, `origin::Probe`, `origin::choose(copied, &Gestures, now, monitor: Rect, &dyn Probe) -> Option<(f32,f32)>`, `Locator::locate(copied, &Gestures, now, monitor)`.

- [ ] Tests: `copied` reads DIB and PNG sizes and reports `CF_HDROP` as `Files`; `choose` follows the spec's order for each type with a fake probe; reversed drag matches; mismatched drag is skipped; off-screen rectangles are ignored by `union_visible`.
- [ ] Run: fails. Implement; locate before `reveal()`; register raw input and handle `WM_INPUT`. Run: passes.
- [ ] Update `Readme.md`.

### Task 4: Verify in the running app

- [ ] `cargo build --release` with no warnings; run it and drive real copies, logging the chosen source against the expected spot.
