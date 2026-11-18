//! Stamps the build with its version and embeds `icon.ico` as the application icon.
//!
//! The version is the local build time, `YYYY.MM.DD.HHMM`, passed to the program
//! as `DRYER_VERSION`. `toolsuild-installer.ps1` sets `DRYER_VERSION` itself so
//! the exe, the installer and the release all carry the same stamp.
//!
//! Instead of calling the Windows resource compiler (rc.exe), this writes the small
//! binary `.res` file by hand and hands it to the MSVC linker, so the build needs no
//! extra tools or crates. Regenerate `icon.ico` from `icon.png` with
//! `tools\make-icon.ps1`.

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

const RT_ICON: u16 = 3;
const RT_GROUP_ICON: u16 = 14;
/// The icon group has this id, so `LoadImageW(.., MAKEINTRESOURCE(1), ..)` finds it,
/// and Explorer uses it as the file icon (the lowest id wins).
const APP_ICON_ID: u16 = 1;

fn main() {
    println!("cargo:rerun-if-changed=icon.ico");
    println!("cargo:rerun-if-changed=build.rs");
    // Restamp whenever the code changes, so a build shows when it was made.
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-env-changed=DRYER_VERSION");
    let version = env::var("DRYER_VERSION")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(build_time)
        .unwrap_or_else(|| env::var("CARGO_PKG_VERSION").unwrap_or_default());
    println!("cargo:rustc-env=DRYER_VERSION={}", version.trim());

    // Only the MSVC linker takes a .res file directly.
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows")
        || env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc")
    {
        return;
    }

    let ico = fs::read("icon.ico").expect("icon.ico is missing; run tools\\make-icon.ps1");
    let res = icon_resources(&ico);
    let path = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR")).join("icon.res");
    fs::write(&path, res).expect("could not write icon.res");
    println!("cargo:rustc-link-arg-bins={}", path.display());
}

/// The local time now as `YYYY.MM.DD.HHMM`. The standard library has no local
/// time zone, so ask PowerShell, which every supported Windows has.
fn build_time() -> Option<String> {
    let out = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", "Get-Date -Format yyyy.MM.dd.HHmm"])
        .output()
        .ok()?;
    let stamp = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (out.status.success() && !stamp.is_empty()).then_some(stamp)
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// One resource record of a .res file, with numeric type and name.
fn resource(out: &mut Vec<u8>, kind: u16, id: u16, data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_le_bytes()); // data size
    out.extend_from_slice(&32u32.to_le_bytes()); // header size
    out.extend_from_slice(&[0xFF, 0xFF]); // type is an ordinal
    out.extend_from_slice(&kind.to_le_bytes());
    out.extend_from_slice(&[0xFF, 0xFF]); // name is an ordinal
    out.extend_from_slice(&id.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // data version
    out.extend_from_slice(&0x1010u16.to_le_bytes()); // movable, pure
    out.extend_from_slice(&0u16.to_le_bytes()); // language: neutral
    out.extend_from_slice(&0u32.to_le_bytes()); // version
    out.extend_from_slice(&0u32.to_le_bytes()); // characteristics
    out.extend_from_slice(data);
    while out.len() % 4 != 0 {
        out.push(0);
    }
}

/// An .ico file becomes one RT_ICON resource per frame plus an RT_GROUP_ICON that
/// lists them, which is how Windows stores icons inside an executable.
fn icon_resources(ico: &[u8]) -> Vec<u8> {
    assert!(ico.len() >= 6 && u16_at(ico, 2) == 1, "icon.ico is not an icon file");
    let count = u16_at(ico, 4) as usize;

    let mut out = Vec::new();
    resource(&mut out, 0, 0, &[]); // every .res file starts with an empty record

    let mut group = Vec::new();
    group.extend_from_slice(&0u16.to_le_bytes());
    group.extend_from_slice(&1u16.to_le_bytes());
    group.extend_from_slice(&(count as u16).to_le_bytes());

    for i in 0..count {
        let entry = 6 + 16 * i;
        let size = u32_at(ico, entry + 8) as usize;
        let offset = u32_at(ico, entry + 12) as usize;
        let frame_id = (i + 1) as u16;
        resource(&mut out, RT_ICON, frame_id, &ico[offset..offset + size]);

        // Directory entry for the group: the .ico's, with the offset replaced by an id.
        group.extend_from_slice(&ico[entry..entry + 12]);
        group.extend_from_slice(&frame_id.to_le_bytes());
    }
    resource(&mut out, RT_GROUP_ICON, APP_ICON_ID, &group);
    out
}
