# Dryer

A clothesline clipboard history for Windows 11.

Dryer has no window. A thin rope hangs across the top of your primary monitor, and everything you copy hangs from it on a clothespin. It keeps the last 10 clips. Click a card to put that clip back on the clipboard as the newest item.

- Text, images and copied files each get a card. Card width is fixed; height fits the content, between a minimum and a maximum.
- The rope sags under the cards, new clips fly up from the pointer, and cards swing from their pegs.
- Clips from password managers that ask not to be recorded are skipped.
- Clips live in memory only; nothing is written to disk.

Built with Rust, Win32, Direct2D and DirectComposition (plus DirectWrite for text).

## Controls

| Action | How |
|---|---|
| Restore a clip | Click its card |
| Show the line | Move the pointer to the very top edge of the screen and rest there or click, copy something, press `Win+Shift+C`, or left-click the tray icon |
| Hide the line | Click anywhere outside the cards; it also retracts by itself after about 6 seconds with nothing happening; or press `Win+Shift+C` / left-click the tray icon |
| Quit | Right-click the tray icon, then Quit |

Only the cards catch the mouse. Everywhere else in the strip, clicks go straight through to the windows underneath. While the line is retracted, Dryer's window is hidden and its graphics memory is released.

Only one instance runs at a time.

## Building

### Requirements

- Windows 10 version 1903 or later (developed on Windows 11)
- [Rust](https://www.rust-lang.org/tools/install) stable, 1.85 or newer (the project uses the 2024 edition)
- The MSVC linker, from [Visual Studio Build Tools](https://visualstudio.microsoft.com/downloads/) with the **Desktop development with C++** workload

Both can be installed with winget:

```powershell
winget install --id Rustlang.Rustup -e
winget install --id Microsoft.VisualStudio.2022.BuildTools -e --override "--quiet --wait --norestart --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
```

Open a new terminal afterwards so `cargo` is on your `PATH`.

### Build and run

```powershell
git clone <this repository>
cd dryer

cargo build --release
.\target\release\dryer.exe
```

Or build and run in one step:

```powershell
cargo run --release
```

The release binary is `target\release\dryer.exe`. It is a single file with no installer, so you can copy it anywhere.

### Tests

```powershell
cargo test
```

### Start with Windows

Put a shortcut to `dryer.exe` in your Startup folder (`Win+R`, then `shell:startup`).

## Tuning the animation

The feel of the motion is set by a few constants at the top of `src\physics.rs`: rope stiffness and sag, swing strength and damping, slide speed, and gust strength and timing. Card sizes and spacing are in `src\card.rs`.

## Project layout

| File | Purpose |
|---|---|
| `src\main.rs` | Window, message loop, tray, hotkey, animation timer |
| `src\clipboard.rs` | Capturing and restoring clipboard formats |
| `src\ring.rs` | The fixed 10-slot history |
| `src\card.rs` | Card sizes, layout along the line, text layout |
| `src\render.rs` | Direct2D drawing and DirectComposition setup |
| `src\physics.rs` | Rope, pendulum, slide and breeze physics |
