//! The beep played when something is copied.
//!
//! The sound is compiled into the executable. Windows' MCI audio API can only play
//! from a file, so it is unpacked once to `%LOCALAPPDATA%\Dryer` and played from
//! there. The player is opened on the first beep and kept open, so later beeps start
//! at once. The on/off choice is remembered by a marker file next to the sound.

use std::fs;
use std::path::PathBuf;

use windows::Win32::Media::Multimedia::mciSendStringW;
use windows::core::PCWSTR;

const BEEP: &[u8] = include_bytes!("../copy-beep.mp3");
const FILE_NAME: &str = "copy-beep.mp3";
/// Present when the sound has been switched off.
const OFF_MARKER: &str = "sound-off";
/// The name MCI knows our player by.
const ALIAS: &str = "dryerbeep";

pub struct Sound {
    dir: Option<PathBuf>,
    enabled: bool,
    /// The MCI player is open.
    open: bool,
    /// Opening failed once; do not keep trying on every copy.
    failed: bool,
}

impl Sound {
    pub fn new() -> Self {
        let dir = std::env::var_os("LOCALAPPDATA").map(|b| PathBuf::from(b).join("Dryer"));
        let enabled = dir.as_ref().is_none_or(|d| !d.join(OFF_MARKER).exists());
        Self { dir, enabled, open: false, failed: false }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Switch the sound on or off, and remember the choice.
    pub fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
        if let Some(dir) = &self.dir {
            let marker = dir.join(OFF_MARKER);
            if on {
                let _ = fs::remove_file(marker);
            } else {
                let _ = fs::create_dir_all(dir);
                let _ = fs::write(marker, b"");
            }
        }
    }

    /// Play the beep from the start (restarting it if it is still playing).
    pub fn play(&mut self) {
        if !self.enabled || self.failed {
            return;
        }
        if !self.open && !self.open_player() {
            self.failed = true;
            return;
        }
        // Playback is asynchronous: both commands return straight away.
        mci(&format!("seek {ALIAS} to start"));
        mci(&format!("play {ALIAS}"));
    }

    /// Unpack the sound if needed and open it in MCI.
    fn open_player(&mut self) -> bool {
        let Some(dir) = &self.dir else { return false };
        let path = dir.join(FILE_NAME);
        let up_to_date = fs::metadata(&path).is_ok_and(|m| m.len() == BEEP.len() as u64);
        if !up_to_date {
            if fs::create_dir_all(dir).is_err() || fs::write(&path, BEEP).is_err() {
                return false;
            }
        }
        let opened = mci(&format!("open \"{}\" type mpegvideo alias {ALIAS}", path.display())) == 0;
        self.open = opened;
        opened
    }
}

impl Drop for Sound {
    fn drop(&mut self) {
        if self.open {
            mci(&format!("close {ALIAS}"));
        }
    }
}

/// Send one MCI command. Returns 0 on success, an MCI error code otherwise.
fn mci(command: &str) -> u32 {
    let wide: Vec<u16> = command.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe { mciSendStringW(PCWSTR(wide.as_ptr()), None, None) }
}
