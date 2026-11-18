# Builds the Windows installer, dist\DryerSetup-<version>.exe, and the portable
# zip, dist\dryer-<version>-windows-x64-portable.zip, with dist\SHA256SUMS.txt.
#
# The version is the build time, YYYY.MM.DD.HHMM (e.g. 2026.11.18.1420). It is
# passed to cargo as DRYER_VERSION, so the tray menu of the exe shows the same one.
#
# Needs Rust (cargo) and Inno Setup 6 (winget install JRSoftware.InnoSetup).
# Run from the repository root:  powershell -File tools\build-installer.ps1
# Use -SkipBuild with -Version to package an exe that was already built with
# `$env:DRYER_VERSION = "<version>"; cargo build --release`.

param([switch]$SkipBuild, [string]$Version)
$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..")

if ($SkipBuild) {
    if (-not $Version) { throw "-SkipBuild needs -Version, the stamp the exe was built with" }
    $version = $Version
} else {
    $version = Get-Date -Format "yyyy.MM.dd.HHmm"
    $env:DRYER_VERSION = $version
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
$zip = "dist\dryer-$version-windows-x64-portable.zip"
Compress-Archive -Path $exe, "Readme.md" -DestinationPath $zip -Force
$sums = foreach ($f in @($setup, $zip)) {
    "$((Get-FileHash $f -Algorithm SHA256).Hash.ToLower())  $(Split-Path $f -Leaf)"
}
Set-Content -Path "dist\SHA256SUMS.txt" -Value $sums -Encoding ascii

"Version $version"
"Built $zip"
"Built $setup ($([math]::Round((Get-Item $setup).Length / 1KB)) KB)"
"SHA256 $hash"