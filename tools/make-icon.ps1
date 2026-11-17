# Builds icon.ico (the application icon) from icon.png.
#
# A Windows icon can hold frames up to 256x256, so the source picture is scaled down
# to each standard size with high-quality resampling and stored as PNG frames.
# Run from the repository root:  powershell -File tools\make-icon.ps1
# Re-run it whenever icon.png changes, then rebuild.

Add-Type -AssemblyName System.Drawing

$source = Join-Path $PSScriptRoot "..\icon.png"
$target = Join-Path $PSScriptRoot "..\icon.ico"
$sizes = 16, 20, 24, 32, 40, 48, 64, 96, 128, 256

$original = [System.Drawing.Image]::FromFile((Resolve-Path $source))
$frames = foreach ($size in $sizes) {
    $bitmap = New-Object System.Drawing.Bitmap $size, $size, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bitmap)
    $g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
    $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::HighQuality
    $g.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
    $g.CompositingQuality = [System.Drawing.Drawing2D.CompositingQuality]::HighQuality
    $g.Clear([System.Drawing.Color]::Transparent)
    $g.DrawImage($original, 0, 0, $size, $size)
    $g.Dispose()
    $stream = New-Object System.IO.MemoryStream
    $bitmap.Save($stream, [System.Drawing.Imaging.ImageFormat]::Png)
    $bitmap.Dispose()
    [pscustomobject]@{ Size = $size; Data = $stream.ToArray() }
}
$original.Dispose()

# ICO layout: header, one 16-byte directory entry per frame, then the PNG frames.
$out = New-Object System.IO.MemoryStream
$w = New-Object System.IO.BinaryWriter $out
$w.Write([uint16]0); $w.Write([uint16]1); $w.Write([uint16]$frames.Count)
$offset = 6 + 16 * $frames.Count
foreach ($f in $frames) {
    $dim = if ($f.Size -ge 256) { 0 } else { $f.Size }   # 0 means 256
    $w.Write([byte]$dim); $w.Write([byte]$dim); $w.Write([byte]0); $w.Write([byte]0)
    $w.Write([uint16]1); $w.Write([uint16]32)
    $w.Write([uint32]$f.Data.Length); $w.Write([uint32]$offset)
    $offset += $f.Data.Length
}
foreach ($f in $frames) { $w.Write($f.Data) }
$w.Flush()
[System.IO.File]::WriteAllBytes($target, $out.ToArray())
"Wrote $target with $($frames.Count) sizes: $($sizes -join ', ')"
