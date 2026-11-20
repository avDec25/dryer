# Dryer

A clothesline clipboard history for Windows 11.

Dryer has no window. A thin rope hangs across the top of your primary monitor, and everything you copy hangs from it on a clothespin. It keeps the last 10 clips. Click a card to put that clip back on the clipboard as the newest item.

- Text, images and copied files each get a card. Card width is fixed; height fits the content, between a minimum and a maximum.
- The rope sags under the cards, new clips fly up from the pointer, and cards swing from their pegs.
- Clips from password managers that ask not to be recorded are skipped.
- A short beep plays whenever a copy is added to the line (the sound is built into the exe). Switch it off with **Copy sound** in the tray menu; the choice is remembered.
- The line is remembered across restarts. Clips are saved in `%LOCALAPPDATA%\Dryer\history`, each in its own file encrypted with Windows DPAPI, so only your account on this PC can read them. Use **Clear history** in the tray menu to erase them.

Built with Rust, Win32, Direct2D and DirectComposition (plus DirectWrite for text).

## Install

Download **DryerSetup-<version>.exe** from the [latest release](https://github.com/avDec25/dryer/releases/latest) and run it.

- It installs for your account only, so it needs no administrator rights. The default folder is `%LOCALAPPDATA%\Programs\Dryer`.
- You can choose to start Dryer when you sign in (this can also be switched later from the tray menu) and to add a desktop shortcut.
- Dryer appears in the Start menu and under Settings, Apps. Uninstalling asks whether to delete your saved clipboard history as well.
- The installer is not code-signed, so Windows SmartScreen may say it protected your PC the first time. Choose **More info**, then **Run anyway**. Each release lists a SHA-256 checksum you can compare with `Get-FileHash DryerSetup-<version>.exe`.
## Controls

| Action | How |
|---|---|
| Restore a clip | Click its card |
| Show the line | Move the pointer to the very top edge of the screen and rest there or click, copy something, press `Win+Shift+C`, or left-click the tray icon |
| Hide the line | Click anywhere outside the cards; it also retracts by itself after about 6 seconds with nothing happening; or press `Win+Shift+C` / left-click the tray icon |
| Reveal only on a click | Right-click the tray icon, then Reveal on click only (checked means the line appears only when you click at the very top edge; resting the pointer there does nothing) |
| Start Dryer when you log in | Right-click the tray icon, then Start with Windows (checked means on) |
| Turn the copy beep on or off | Right-click the tray icon, then Copy sound (checked means on) |
| Erase all saved clips | Right-click the tray icon, then Clear history |
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

### Building the installer

The installer is made with [Inno Setup 6](https://jrsoftware.org/isinfo.php) (`winget install JRSoftware.InnoSetup`):

```powershell
powershell -File tools\build-installer.ps1
```

This builds the release exe, then writes `dist\DryerSetup-<version>.exe` (with its `.sha256` file), the portable zip `dist\dryer-<version>-windows-x64-portable.zip` and `dist\SHA256SUMS.txt`. The version is the build time, `YYYY.MM.DD.HHMM` (for example `2026.11.18.1420`), and the tray menu shows it at the top. The installer definition is `installer\dryer.iss`.
## Changing the icon

Replace `icon.png` (a square picture, 256 px or larger), run `powershell -File tools\make-icon.ps1` to rebuild `icon.ico` from it, then rebuild. The icon is embedded in the exe, used for the tray icon, and shown by Explorer and Task Manager.

## Tuning the animation

The feel of the motion is set by a few constants at the top of `src\physics.rs`: rope stiffness and sag, swing strength and damping, slide speed, and gust strength and timing. Card sizes and spacing are in `src\card.rs`.

## Project layout

| File | Purpose |
|---|---|
| `src\main.rs` | Window, message loop, tray, hotkey, animation timer |
| `src\clipboard.rs` | Capturing and restoring clipboard formats |
| `src\ring.rs` | The fixed 10-slot history |
| `src\sound.rs` | The copy beep (`copy-beep.mp3`, embedded in the exe) |
| `src\store.rs` | Saving the history to disk (encrypted) and loading it at startup |
| `installer\dryer.iss` | The Inno Setup script for the installer |
| `tools\build-installer.ps1` | Builds the release exe and the installer into `dist\` |
| `build.rs` | Embeds `icon.ico` in the exe as the application icon (no extra tools needed) |
| `icon.png` / `icon.ico` | The application icon, and the multi-size version of it built from the PNG |
| `tools\make-icon.ps1` | Rebuilds `icon.ico` from `icon.png` |
| `src\autostart.rs` | Start at login (the per-user `Run` registry key) |
| `src\card.rs` | Card sizes, layout along the line, text layout |
| `src\render.rs` | Direct2D drawing and DirectComposition setup |
| `src\physics.rs` | Rope, pendulum, slide and breeze physics |
