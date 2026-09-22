<#
Build assets\experience.ico: a multi-size (16/32/48/256, PNG-compressed) icon
for the two desktop shortcuts. The design is deliberately simple — a rounded
indigo tile with a white check — because it has to read at 16px and must not
look like a shell script.

Run: powershell -NoProfile -ExecutionPolicy Bypass -File assets\make-icon.ps1
#>
param([string]$Out = (Join-Path $PSScriptRoot "experience.ico"))
$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Drawing

function New-Tile([int]$size) {
    $bmp = New-Object System.Drawing.Bitmap($size, $size)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
    $g.Clear([System.Drawing.Color]::Transparent)

    # rounded background
    $radius = [Math]::Max(2, [int]($size * 0.22))
    $path = New-Object System.Drawing.Drawing2D.GraphicsPath
    $d = $radius * 2
    $path.AddArc(0, 0, $d, $d, 180, 90)
    $path.AddArc($size - $d, 0, $d, $d, 270, 90)
    $path.AddArc($size - $d, $size - $d, $d, $d, 0, 90)
    $path.AddArc(0, $size - $d, $d, $d, 90, 90)
    $path.CloseFigure()
    $brush = New-Object System.Drawing.SolidBrush(
        [System.Drawing.Color]::FromArgb(255, 49, 46, 129))
    $g.FillPath($brush, $path)

    # white check mark: two strokes, scaled to the tile
    $pen = New-Object System.Drawing.Pen([System.Drawing.Color]::White, [Math]::Max(1.5, $size * 0.13))
    $pen.StartCap = [System.Drawing.Drawing2D.LineCap]::Round
    $pen.EndCap = [System.Drawing.Drawing2D.LineCap]::Round
    $pen.LineJoin = [System.Drawing.Drawing2D.LineJoin]::Round
    $points = @(
        (New-Object System.Drawing.PointF([single]($size * 0.24), [single]($size * 0.54))),
        (New-Object System.Drawing.PointF([single]($size * 0.43), [single]($size * 0.72))),
        (New-Object System.Drawing.PointF([single]($size * 0.77), [single]($size * 0.30)))
    )
    $g.DrawLines($pen, $points)

    $g.Dispose(); $brush.Dispose(); $pen.Dispose(); $path.Dispose()
    return $bmp
}

$sizes = @(16, 32, 48, 256)
$images = @()
foreach ($size in $sizes) {
    $bmp = New-Tile $size
    $stream = New-Object System.IO.MemoryStream
    $bmp.Save($stream, [System.Drawing.Imaging.ImageFormat]::Png)
    $images += , @{ Size = $size; Bytes = $stream.ToArray() }
    $stream.Dispose(); $bmp.Dispose()
}

# ICO container: header + one directory entry per image, PNG payloads.
$outStream = New-Object System.IO.MemoryStream
$writer = New-Object System.IO.BinaryWriter($outStream)
$writer.Write([UInt16]0)                 # reserved
$writer.Write([UInt16]1)                 # type: icon
$writer.Write([UInt16]$images.Count)
$offset = 6 + 16 * $images.Count
foreach ($image in $images) {
    $dimension = if ($image.Size -ge 256) { 0 } else { $image.Size }
    $writer.Write([Byte]$dimension)      # width (0 = 256)
    $writer.Write([Byte]$dimension)      # height
    $writer.Write([Byte]0)               # palette
    $writer.Write([Byte]0)               # reserved
    $writer.Write([UInt16]1)             # colour planes
    $writer.Write([UInt16]32)            # bits per pixel
    $writer.Write([UInt32]$image.Bytes.Length)
    $writer.Write([UInt32]$offset)
    $offset += $image.Bytes.Length
}
foreach ($image in $images) { $writer.Write($image.Bytes) }
$writer.Flush()
[System.IO.File]::WriteAllBytes($Out, $outStream.ToArray())
$writer.Dispose(); $outStream.Dispose()

$info = Get-Item $Out
Write-Host ("wrote {0} ({1} bytes, sizes {2})" -f $info.FullName, $info.Length, ($sizes -join "/"))
