param(
    [string]$OutIcoPath = (Join-Path $PSScriptRoot "icon.ico")
)

Add-Type -AssemblyName System.Drawing

function Create-LocalShotIcon {
    param([string]$TargetIcoPath)

    $sizes = @(256, 64, 48, 32, 16)
    $pngBytesList = @()

    foreach ($sz in $sizes) {
        $bmp = New-Object System.Drawing.Bitmap($sz, $sz, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
        $g = [System.Drawing.Graphics]::FromImage($bmp)
        $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
        $g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
        $g.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality

        # Rounded rectangle background with gradient
        $rect = New-Object System.Drawing.Rectangle(0, 0, $sz, $sz)
        $color1 = [System.Drawing.Color]::FromArgb(79, 70, 229)   # #4F46E5
        $color2 = [System.Drawing.Color]::FromArgb(219, 39, 119)  # #DB2777
        $brush = New-Object System.Drawing.Drawing2D.LinearGradientBrush($rect, $color1, $color2, 45.0)
        
        $radius = [int]($sz * 0.22)
        $path = New-Object System.Drawing.Drawing2D.GraphicsPath
        $d = $radius * 2
        $m = [int]($sz * 0.06)
        $w = $sz - ($m * 2)
        $path.AddArc($m, $m, $d, $d, 180, 90)
        $path.AddArc($m + $w - $d, $m, $d, $d, 270, 90)
        $path.AddArc($m + $w - $d, $m + $w - $d, $d, $d, 0, 90)
        $path.AddArc($m, $m + $w - $d, $d, $d, 90, 90)
        $path.CloseFigure()
        
        $g.FillPath($brush, $path)

        # Draw Lens
        $lensMargin = [int]($sz * 0.26)
        $lensSize = $sz - ($lensMargin * 2)
        $lensRect = New-Object System.Drawing.Rectangle($lensMargin, $lensMargin, $lensSize, $lensSize)
        $lensColor1 = [System.Drawing.Color]::FromArgb(56, 189, 248) # #38BDF8
        $lensColor2 = [System.Drawing.Color]::FromArgb(3, 105, 161)  # #0369A1
        $lensBrush = New-Object System.Drawing.Drawing2D.LinearGradientBrush($lensRect, $lensColor1, $lensColor2, 90.0)
        $g.FillEllipse($lensBrush, $lensRect)

        # Lens center aperture
        $centerM = [int]($sz * 0.38)
        $centerSz = $sz - ($centerM * 2)
        $centerBrush = New-Object System.Drawing.SolidBrush([System.Drawing.Color]::FromArgb(15, 23, 42))
        $g.FillEllipse($centerBrush, $centerM, $centerM, $centerSz, $centerSz)

        # White corner crop marks
        $penWidth = [Math]::Max(1.0, $sz * 0.04)
        $whitePen = New-Object System.Drawing.Pen([System.Drawing.Color]::White, $penWidth)
        $cOffset = [int]($sz * 0.22)
        $cLen = [int]($sz * 0.12)
        
        # Top-left
        $g.DrawLine($whitePen, $cOffset, $cOffset + $cLen, $cOffset, $cOffset)
        $g.DrawLine($whitePen, $cOffset, $cOffset, $cOffset + $cLen, $cOffset)
        # Top-right
        $g.DrawLine($whitePen, $sz - $cOffset - $cLen, $cOffset, $sz - $cOffset, $cOffset)
        $g.DrawLine($whitePen, $sz - $cOffset, $cOffset, $sz - $cOffset, $cOffset + $cLen)
        # Bottom-left
        $g.DrawLine($whitePen, $cOffset, $sz - $cOffset - $cLen, $cOffset, $sz - $cOffset)
        $g.DrawLine($whitePen, $cOffset, $sz - $cOffset, $cOffset + $cLen, $sz - $cOffset)
        # Bottom-right
        $g.DrawLine($whitePen, $sz - $cOffset - $cLen, $sz - $cOffset, $sz - $cOffset, $sz - $cOffset)
        $g.DrawLine($whitePen, $sz - $cOffset, $sz - $cOffset - $cLen, $sz - $cOffset, $sz - $cOffset)

        $g.Dispose()

        $ms = New-Object System.IO.MemoryStream
        $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
        $pngBytesList += ,@($sz, $ms.ToArray())
        $bmp.Dispose()
        $ms.Dispose()
    }

    # Build ICO file structure
    $fs = [System.IO.File]::Create($TargetIcoPath)
    $bw = New-Object System.IO.BinaryWriter($fs)

    # Header: Reserved (0), Type (1 = ICO), Count
    $bw.Write([uint16]0)
    $bw.Write([uint16]1)
    $bw.Write([uint16]$pngBytesList.Count)

    # Offset starts after header (6) + entries (16 * count)
    $offset = 6 + (16 * $pngBytesList.Count)

    # Write directory entries
    foreach ($item in $pngBytesList) {
        $sz = $item[0]
        $bytes = $item[1]
        $bWidth = if ($sz -ge 256) { 0 } else { [byte]$sz }
        $bHeight = if ($sz -ge 256) { 0 } else { [byte]$sz }

        $bw.Write([byte]$bWidth)        # Width
        $bw.Write([byte]$bHeight)       # Height
        $bw.Write([byte]0)              # Color count
        $bw.Write([byte]0)              # Reserved
        $bw.Write([uint16]1)            # Color planes
        $bw.Write([uint16]32)           # Bits per pixel
        $bw.Write([uint32]$bytes.Length)# Image data size
        $bw.Write([uint32]$offset)      # Offset of image data

        $offset += $bytes.Length
    }

    # Write PNG image datas
    foreach ($item in $pngBytesList) {
        $bytes = $item[1]
        $bw.Write($bytes)
    }

    $bw.Flush()
    $bw.Close()
    $fs.Close()
    Write-Output "ICO successfully created at $TargetIcoPath"
}

Create-LocalShotIcon -TargetIcoPath $OutIcoPath
