# filmstrip_badge_probe.ps1 - screenshot the thumbnail strip so the per-cell index
# badge (m260) is inspected as pixels instead of "looks right on my machine".
#
# What it is for: user asked for a number in the bottom-right corner of every cell
# ("so the gap between pictures is obvious at a glance"). m261 changed the look:
# the digit itself has a 1px white outline (eight offset white draws, then one
# black draw on top), with NO filled plate behind it. A pure geometry unit test
# cannot see whether the digits actually reach the screen, so this rig paints
# the real window and dumps one PNG.
# Also useful for the m261 scroll-bar move: the 6dp track now sits at the TOP of
# the bottom strip (above the cells), away from the window resize border.
#
# Rules honoured: ASCII only, private exe root with a pinned ini, touches only the
# instance it starts, closes it with WM_CLOSE.

param(
    [Parameter(Position = 0)][string]$Image = 'E:\tmp\AI.Agent\RuPix\.build\1M\noise.png',
    [string]$Tag = 'badge',
    [int]$WindowW = 800,
    [int]$WindowH = 600,
    [int]$Left = 1,
    [int]$WheelSteps = 0
)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class FB {
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, int m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr h, ref POINT p);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L; public int T; public int R; public int B; }
  [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X; public int Y; }
}
"@

$build = 'E:\tmp\AI.Agent\RuPix\.build'
$srcExe = Join-Path $build 'rupix.exe'
$exeRoot = "E:\tmp\AI.Agent\RuPix\.temp\probe-$Tag"
New-Item -ItemType Directory -Force -Path $exeRoot | Out-Null
$exe = Join-Path $exeRoot 'rupix.exe'
if (-not (Test-Path $exe)) {
    try { New-Item -ItemType HardLink -Path $exe -Target $srcExe -ErrorAction Stop | Out-Null }
    catch { Copy-Item $srcExe $exe -Force }
}
$env:RUPIX_DECODERS_DIR = Join-Path $build 'decoders'
@(
    'prefetch=1', 'close_to_tray=0', 'minimal_mode=0',
    'decoders_preferred=', 'decoders_disabled=',
    ('window_w={0}' -f $WindowW), ('window_h={0}' -f $WindowH), 'init_window=0',
    'opacity=100', 'checker=0', 'stay_on_top=0',
    'filmstrip=1', ('filmstrip_left={0}' -f $Left)
) | Set-Content -Path (Join-Path $exeRoot 'rupix.ini') -Encoding ascii

$pre = @(Get-Process -Name rupix -ErrorAction SilentlyContinue)
if ($pre.Count -gt 0) { Write-Output 'ABORT_A_RUPIX_IS_RUNNING'; exit 1 }
if (-not (Test-Path $srcExe)) { Write-Output 'NO_EXE'; exit 1 }
if (-not (Test-Path $Image)) { Write-Output 'NO_IMAGE'; exit 1 }

$p = Start-Process -FilePath $exe -ArgumentList "`"$Image`"" -PassThru -WorkingDirectory $exeRoot
$h = [IntPtr]::Zero
for ($i = 0; $i -lt 60; $i++) {
    Start-Sleep -Milliseconds 200
    $p.Refresh()
    $h = $p.MainWindowHandle
    if ($h -ne [IntPtr]::Zero) { break }
}
if ($h -eq [IntPtr]::Zero) { Write-Output 'ABORT_NO_WINDOW'; exit 1 }
# Thumbnails are decoded on a worker thread; give the strip time to fill in so the
# digits are drawn on top of real pictures rather than on empty plates.
Start-Sleep -Seconds 4

if ($WheelSteps -ne 0) {
    # Feed real WM_MOUSEWHEEL into the strip's own area (client x=56 sits inside a
    # cell, y=260 in the middle of the column). The framework runs ScreenToClient on
    # the message coords, so lParam must carry SCREEN coordinates.
    #
    # Why this matters: m260 removed "pull the strip back to the current picture".
    # If that rule were still in place, these steps would scroll the strip away and
    # the very next frame would snap it back - the screenshot would show the same
    # first number as before.
    $pt = [FB+POINT]::new()
    $pt.X = 56
    $pt.Y = 260
    [void][FB]::ClientToScreen($h, [ref]$pt)
    $d = if ($WheelSteps -gt 0) { 120 } else { -120 }
    [int64]$wp = [int64](($d -band 0xFFFF) -shl 16)
    [int64]$lp = ([int64](($pt.Y -band 0xFFFF) -shl 16)) -bor [int64]($pt.X -band 0xFFFF)
    for ($i = 0; $i -lt [Math]::Abs($WheelSteps); $i++) {
        [void][FB]::PostMessage($h, 0x020A, [IntPtr]$wp, [IntPtr]$lp)
        Start-Sleep -Milliseconds 150
    }
    Start-Sleep -Milliseconds 800
    Write-Output ("WHEEL {0} steps at screen {1},{2}" -f $WheelSteps, $pt.X, $pt.Y)
}

$rc = [FB+RECT]::new()
[void][FB]::GetClientRect($h, [ref]$rc)
[int]$w = $rc.R - $rc.L
[int]$ht = $rc.B - $rc.T
Write-Output ("CLIENT {0}x{1}" -f $w, $ht)

try {
    $bmp = [System.Drawing.Bitmap]::new($w, $ht)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $hdc = $g.GetHdc()
    [void][FB]::PrintWindow($h, $hdc, 2)
    $g.ReleaseHdc($hdc)
    $out = "E:\tmp\AI.Agent\RuPix\.temp\strip_{0}.png" -f $Tag
    $bmp.Save($out, [System.Drawing.Imaging.ImageFormat]::Png)
    # The digit is about 10dp tall - unreadable at 1:1 in any screenshot review, so
    # dump a 4x nearest-neighbour zoom of the strip as well. Follow the strip side:
    # bottom strip -> bottom band, left strip -> left column (m261).
    [int]$cropX = 0
    [int]$cropY = 0
    if ($Left -eq 0) {
        [int]$cropW = [Math]::Min(600, $w)
        [int]$cropH = [Math]::Min(120, $ht)
        $cropY = $ht - $cropH
    } else {
        [int]$cropW = [Math]::Min(130, $w)
        [int]$cropH = [Math]::Min(320, $ht)
    }
    $zoom = New-Object System.Drawing.Bitmap ($cropW * 4), ($cropH * 4)
    $zg = [System.Drawing.Graphics]::FromImage($zoom)
    $zg.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::NearestNeighbor
    $zg.DrawImage(
        $bmp,
        (New-Object System.Drawing.Rectangle 0, 0, ($cropW * 4), ($cropH * 4)),
        (New-Object System.Drawing.Rectangle $cropX, $cropY, $cropW, $cropH),
        [System.Drawing.GraphicsUnit]::Pixel
    )
    $zg.Dispose()
    $zout = "E:\tmp\AI.Agent\RuPix\.temp\strip_{0}_zoom.png" -f $Tag
    $zoom.Save($zout, [System.Drawing.Imaging.ImageFormat]::Png)
    $zoom.Dispose()
    $g.Dispose()
    $bmp.Dispose()
    Write-Output ("CAPTURED {0}" -f $out)
    Write-Output ("CAPTURED {0}" -f $zout)
} catch {
    Write-Output ("FAILED {0}" -f $_.Exception.Message)
}

[void][FB]::PostMessage($h, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero)
for ($i = 0; $i -lt 200; $i++) {
    Start-Sleep -Milliseconds 20
    $p.Refresh()
    if ($p.HasExited) { break }
}
Write-Output ("EXITED={0}" -f $p.HasExited)
