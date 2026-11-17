# Builds the Windows installer: dist\DryerSetup-<version>.exe
#
# Needs Rust (cargo) and Inno Setup 6 (winget install JRSoftware.InnoSetup).
# Run from the repository root:  powershell -File tools\build-installer.ps1
# Use -SkipBuild to package an exe that was already built with `cargo build --release`.

param([switch]$SkipBuild)
$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..")

$version = ((Get-Content Cargo.toml | Select-String '^version\s*=\s*"([^"]+)"').Matches[0].Groups[1].Value)

if (-not $SkipBuild) {
    cargo build --release
    if ($LASTEXITCODE) { throw "cargo build failed" }
}
$targetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { "target" }
$exe = (Resolve-Path (Join-Path $targetDir "release\dryer.exe")).Path

$candidates = @(
    (Get-Command ISCC.exe -ErrorAction SilentlyContinue).Source,
    "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe",
    "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
    "$env:ProgramFiles\Inno Setup 6\ISCC.exe"
)
$iscc = $candidates | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
if (-not $iscc) { throw "Inno Setup 6 not found. Install it with: winget install JRSoftware.InnoSetup" }

& $iscc "/DAppVersion=$version" "/DExePath=$exe" "installer\dryer.iss"
if ($LASTEXITCODE) { throw "Inno Setup failed" }

$setup = "dist\DryerSetup-$version.exe"
$hash = (Get-FileHash $setup -Algorithm SHA256).Hash.ToLower()
Set-Content -Path "$setup.sha256" -Value "$hash  DryerSetup-$version.exe" -Encoding ascii
"Built $setup ($([math]::Round((Get-Item $setup).Length / 1KB)) KB)"
"SHA256 $hash"