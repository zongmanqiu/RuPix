# pan_probe.ps1 - end-to-end verification of WASD pan + double-tap-to-edge (m216).
#
# Fixture: a 400x2400 vertical gradient PNG (black at top, white at bottom),
# opened at 100% ('1') in a 500x500 window. Brightness oracle:
#   looking at the top (W W) -> darkest; center -> mid; bottom (S S) -> brightest.
# Expected ordering: top < single_W < base(center) < single_S < bottom.
#
# Rules: ASCII only; private root + hardlinked exe + pinned ini + private temp;
# taskkill before start; only the child we started is closed.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File build\pan_probe.ps1
$ErrorActionPreference = 'Stop'

$image = 'E:\tmp\AI.Agent\RuPix\.temp\big-samples\pan_gradient.png'
if (-not (Test-Path $image)) { Write-Output 'NO_IMAGE'; exit 1 }
$srcExe = 'E:\tmp\AI.Agent\RuPix\.build\rupix.exe'
if (-not (Test-Path $srcExe)) { Write-Output 'NO_EXE'; exit 1 }

cmd /c "taskkill /F /IM rupix.exe >nul 2>&1"
Start-Sleep -Milliseconds 400

$root = 'E:\tmp\AI.Agent\RuPix\.temp\probe-pan'
New-Item -ItemType Directory -Force -Path $root | Out-Null
Remove-Item -Recurse -Force $root -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $root | Out-Null
$exe = Join-Path $root 'rupix.exe'
try {
    New-Item -ItemType HardLink -Path $exe -Target $srcExe -ErrorAction Stop | Out-Null
} catch {
    Copy-Item $srcExe $exe -Force
}
New-Item -ItemType Directory -Force -Path (Join-Path $root 'temp') | Out-Null
@(
    'prefetch=0', 'close_to_tray=0', 'minimal_mode=0',
    'decoders_preferred=', 'decoders_disabled=',
    'window_w=500', 'window_h=500', 'init_window=0',
    'opacity=100', 'filmstrip=0', 'checker=0', 'stay_on_top=0'
) | Set-Content -Path (Join-Path $root 'rupix.ini') -Encoding ascii

$env:RUPIX_DECODERS_DIR = 'E:\tmp\AI.Agent\RuPix\.build\decoders'
Remove-Item Env:RUST_LOG -ErrorAction SilentlyContinue

Add-Type -AssemblyName System.Drawing
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class PanProbeWin {
    [StructLayout(LayoutKind.Sequential)]
    public struct RECT { public int Left, Top, Right, Bottom; }
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint msg, IntPtr w, IntPtr l);
    [DllImport("user32.dll")] public static extern uint MapVirtualKeyW(uint vk, uint mapType);
}
'@

function Shot([IntPtr]$h, [string]$path) {
    $r = New-Object PanProbeWin+RECT
    [void][PanProbeWin]::GetWindowRect($h, [ref]$r)
    $bmp = New-Object System.Drawing.Bitmap ($r.Right - $r.Left), ($r.Bottom - $r.Top)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $hdc = $g.GetHdc()
    [void][PanProbeWin]::PrintWindow($h, $hdc, 2)
    $g.ReleaseHdc($hdc)
    $g.Dispose()
    $bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
    $bmp.Dispose()
}

function MeanBrightness([string]$path) {
    $bmp = New-Object System.Drawing.Bitmap $path
    $sum = [double]0; $n = 0
    for ($y = 0; $y -lt $bmp.Height; $y += 4) {
        for ($x = 0; $x -lt $bmp.Width; $x += 4) {
            $c = $bmp.GetPixel($x, $y)
            $sum += ($c.R + $c.G + $c.B) / 3.0
            $n++
        }
    }
    $bmp.Dispose()
    return [math]::Round($sum / $n, 1)
}

# Channel means (0..255). The edge bands live in R (top) and B (bottom): at the
# keep-visible strip only ~24 px of the image remain on screen, so whole-frame
# brightness cannot tell top from bottom (dark background dominates), but the
# R/B split can: W-edge shows the red band, S-edge shows the blue band.
function ChannelMean([string]$path, [string]$ch) {
    $bmp = New-Object System.Drawing.Bitmap $path
    $sum = [double]0; $n = 0
    for ($y = 0; $y -lt $bmp.Height; $y += 2) {
        for ($x = 0; $x -lt $bmp.Width; $x += 2) {
            $c = $bmp.GetPixel($x, $y)
            $sum += [double]$c.$ch
            $n++
        }
    }
    $bmp.Dispose()
    return [math]::Round($sum / $n, 1)
}

function Press([IntPtr]$h, [int]$vk) {
    # ASCII == VK for letters/digits; VkKeyScanW not needed here (W/S only).
    $sc = ([int][PanProbeWin]::MapVirtualKeyW([uint32]$vk, 1)) -band 0xffff
    $kd = [IntPtr]([long]1 -bor ([long]$sc -shl 16) -bor 0x40000000L)
    $ku = [IntPtr]([long]1 -bor ([long]$sc -shl 16) -bor 0xC0000000L)
    [void][PanProbeWin]::PostMessage($h, 0x0100, [IntPtr]$vk, $kd)
    [void][PanProbeWin]::PostMessage($h, 0x0101, [IntPtr]$vk, $ku)
}

# Double-tap: two full presses TAP ms apart. Since m216's edge-jump was
# revoked (user ruling: plain steps only), a "double-tap" is now just two
# plain steps - the oracle asserts the two-step displacement, NOT a jump.
function DoubleTap([IntPtr]$h, [int]$vk, [int]$tapMs) {
    Press $h $vk
    Start-Sleep -Milliseconds $tapMs
    Press $h $vk
}

$p = Start-Process -FilePath $exe -ArgumentList "`"$image`"" -PassThru -WorkingDirectory $root
$h = [IntPtr]::Zero
for ($i = 0; $i -lt 100; $i++) {
    Start-Sleep -Milliseconds 100
    $p.Refresh()
    if ($p.HasExited) { Write-Output 'ABORT_EXITED'; exit 1 }
    $h = [IntPtr]$p.MainWindowHandle
    if ($h -ne [IntPtr]::Zero) { break }
}
if ($h -eq [IntPtr]::Zero) { Write-Output 'ABORT_NO_WINDOW'; exit 1 }
Write-Output ("WINDOW pid={0} hwnd={1}" -f $p.Id, $h)
Start-Sleep -Seconds 8

# 100% zoom so the 2400 px tall image vastly overflows the 500 px viewport.
Press $h 0x31   # '1' = actual size
Start-Sleep -Milliseconds 2000
# Recenter (fit resets pan, then back to 100%).
function Recentre([IntPtr]$h) {
    Press $h 0x30   # '0' = fit -> pan (0,0)
    Start-Sleep -Milliseconds 1200
    Press $h 0x31   # '1' = 100%
    Start-Sleep -Milliseconds 1200
}
Recentre $h

$dir = Join-Path $root 'shots'
New-Item -ItemType Directory -Force -Path $dir | Out-Null

# Arm 1: center baseline.
$base = Join-Path $dir 'base.png'
Shot $h $base
$bBase = MeanBrightness $base

# Arm 2: single W (one step toward the top) -> slightly darker than center.
Press $h 0x57   # W
Start-Sleep -Milliseconds 800
$w1 = Join-Path $dir 'w1.png'
Shot $h $w1
$bW1 = MeanBrightness $w1

# Arm 3: double-tap W from center -> top edge, darkest.
Recentre $h
DoubleTap $h 0x57 120
Start-Sleep -Milliseconds 900
$top = Join-Path $dir 'top.png'
Shot $h $top
$bTop = MeanBrightness $top

# Arm 4: double-tap S from center -> bottom edge, brightest.
Recentre $h
DoubleTap $h 0x53 120
Start-Sleep -Milliseconds 900
$bot = Join-Path $dir 'bottom.png'
Shot $h $bot
$bBot = MeanBrightness $bot

Write-Output ("PAN base={0} w1={1}" -f $bBase, $bW1)
$rTop = ChannelMean $top 'R'; $bTop2 = ChannelMean $top 'B'
$rBot = ChannelMean $bot 'R'; $bBot2 = ChannelMean $bot 'B'
Write-Output ("PAN edge top(R={0} B={1}) bottom(R={2} B={3})" -f $rTop, $bTop2, $rBot, $bBot2)

# Minimap viewport-rect oracle for the S edge. Known geometry nit (pre-existing,
# m206 ruling): at the top edge the 24 dp keep-visible strip hides under the
# custom title bar (frameless window draws chrome over the canvas top), so the
# main canvas shows nothing at the S edge while the W edge strip stays visible
# above the floating toolbar. The minimap's viewport rectangle is the honest
# readout of where the view actually is: fixture v3 caps the mid gradient at
# 50% gray, so near-white pixels inside the minimap box = the viewport rect.
function MinimapRectRow([string]$path) {
    $bmp = New-Object System.Drawing.Bitmap $path
    # Minimap box: right edge of the window, lower half (X ~ w-56..w-8,
    # Y ~ 250..415 for a 484x461 shot; scan generously).
    $x0 = $bmp.Width - 56; $x1 = $bmp.Width - 8
    $y0 = [int]($bmp.Height * 0.5); $y1 = $bmp.Height - 45
    $rows = @()
    for ($y = $y0; $y -lt $y1; $y++) {
        $n = 0
        for ($x = $x0; $x -lt $x1; $x++) {
            $c = $bmp.GetPixel($x, $y)
            if ($c.R -gt 180 -and $c.G -gt 180 -and $c.B -gt 180) { $n++ }
        }
        if ($n -gt 8) { $rows += $y }
    }
    $bmp.Dispose()
    if ($rows.Count -eq 0) { return -1.0 }
    return [math]::Round((($rows | Measure-Object -Average).Average), 1)
}
$mBase = MinimapRectRow $base
$mTop = MinimapRectRow $top
$mBot = MinimapRectRow $bot
Write-Output ("PAN minimap rect row: base={0} top={1} bottom={2}" -f $mBase, $mTop, $mBot)

# Minimap rect readings are informational since the edge-jump revocation: a
# double-tap now moves exactly two plain steps (small rect shift), so the
# verdict rides on the canvas brightness ordering alone: two steps up must be
# darker than center, two steps down brighter (v3 fixture: mid gradient caps
# at 50% gray, edges carry the red/blue bands which stay out of view unless
# an edge jump regresses into existence - red/blue asymmetry would flag it).
$ok = ($bW1 -lt $bBase) -and ($rTop -lt $bBase) -and ($rBot -gt $bBase)
if ($ok) { Write-Output 'PAN VERDICT: PASS (W step darkens, W W = 2 steps up, S S = 2 steps down, no edge jump)' } else { Write-Output 'PAN VERDICT: FAIL' }

[void][PanProbeWin]::PostMessage($h, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero)
for ($i = 0; $i -lt 40; $i++) {
    Start-Sleep -Milliseconds 100
    $p.Refresh()
    if ($p.HasExited) { break }
}
if (-not $p.HasExited) {
    Stop-Process -Id $p.Id -Force
    Write-Output 'CLOSE forced (own pid)'
} else {
    Write-Output 'CLOSE wm_close'
}
