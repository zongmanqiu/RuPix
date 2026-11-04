# key_dispatch_probe.ps1 - count how many actions ONE posted character key fires.
#
# Oracle for todo section 22 (double dispatch: WM_KEYDOWN + TranslateMessage's
# WM_CHAR both reaching match_action). Fixture: an alternating black/white GIF
# (frame 1 = black, 2 = white, ...), frame delay 300 s so autoplay cannot move.
# Start on black; after ONE '.' press:
#   exactly 1 action  -> white  (single dispatch, section 22 is stale)
#   0 or 2 actions    -> black  (ambiguous; resolved by log reading below)
# Log reading disambiguates: RUST_LOG=trace captures windui's KEYDOWN dispatch
# line "key Other(190) ..." (one per KEYDOWN reaching handle_key).
#
# Rules: ASCII only; private root + hardlinked exe + pinned ini + private temp;
# taskkill before start (single-instance mutex is global); only the child we
# started is closed (WM_CLOSE, then own-pid fallback).
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File build\key_dispatch_probe.ps1
#   powershell -ExecutionPolicy Bypass -File build\key_dispatch_probe.ps1 -Key ','
#   powershell -ExecutionPolicy Bypass -File build\key_dispatch_probe.ps1 -SendCharDirect

param(
    [string]$Key = '.',
    [switch]$SendCharDirect,
    [string]$Image = 'E:\tmp\AI.Agent\RuPix\.temp\big-samples\keydisp_alternating.gif',
    [int]$SettleSeconds = 8
)
$ErrorActionPreference = 'Stop'

if (-not (Test-Path $Image)) { Write-Output 'NO_IMAGE'; exit 1 }
$srcExe = 'E:\tmp\AI.Agent\RuPix\.build\rupix.exe'
if (-not (Test-Path $srcExe)) { Write-Output 'NO_EXE'; exit 1 }

cmd /c "taskkill /F /IM rupix.exe >nul 2>&1"
Start-Sleep -Milliseconds 400

$root = 'E:\tmp\AI.Agent\RuPix\.temp\probe-keydisp'
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
$env:RUST_LOG = 'trace'
Remove-Item Env:RUPIX_DECODE_TRACE -ErrorAction SilentlyContinue

Add-Type -AssemblyName System.Drawing
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class KeyProbeWin {
    [StructLayout(LayoutKind.Sequential)]
    public struct RECT { public int Left, Top, Right, Bottom; }
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint msg, IntPtr w, IntPtr l);
    [DllImport("user32.dll")] public static extern uint MapVirtualKeyW(uint vk, uint mapType);
    [DllImport("user32.dll")] public static extern short VkKeyScanW(char ch);
}
'@

function Shot([IntPtr]$h, [string]$path) {
    $r = New-Object KeyProbeWin+RECT
    [void][KeyProbeWin]::GetWindowRect($h, [ref]$r)
    $bmp = New-Object System.Drawing.Bitmap ($r.Right - $r.Left), ($r.Bottom - $r.Top)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $hdc = $g.GetHdc()
    [void][KeyProbeWin]::PrintWindow($h, $hdc, 2)
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
    if ($n -eq 0) { return -1.0 }
    return [math]::Round($sum / $n, 1)
}

function Get-Vk([string]$ch) {
    # ASCII == VK only for letters/digits; punctuation needs VkKeyScanW
    # ('.' is 46 as ASCII but VK_OEM_PERIOD=190; 46 is VK_DELETE!).
    if ($ch -match '^[A-Za-z0-9]$') { return [int][char]$ch.ToUpperInvariant() }
    $r = [KeyProbeWin]::VkKeyScanW([char]$ch)
    if ($r -eq -1) { throw "no VK for '$ch'" }
    return ($r -band 0xFF)
}

$p = Start-Process -FilePath $exe -ArgumentList "`"$Image`"" -PassThru -WorkingDirectory $root
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
Start-Sleep -Seconds $SettleSeconds

$shotBefore = Join-Path $root 'before.png'
$shotAfter = Join-Path $root 'after.png'
Shot $h $shotBefore
$brightBefore = MeanBrightness $shotBefore

if ($SendCharDirect) {
    # Arm B: post WM_CHAR only (skips the KEYDOWN leg entirely). WM_CHAR's
    # wparam is the UTF-16 code unit (46 for '.'), NOT the virtual key.
    [void][KeyProbeWin]::PostMessage($h, 0x0102, [IntPtr][int][char]$Key, [IntPtr]::Zero)
} else {
    # Arm A: post WM_KEYDOWN + WM_KEYUP like a real key press; the app's own
    # TranslateMessage generates the WM_CHAR.
    $vk = Get-Vk $Key
    $sc = ([int][KeyProbeWin]::MapVirtualKeyW([uint32]$vk, 1)) -band 0xffff
    $kd = [IntPtr]([long]1 -bor ([long]$sc -shl 16) -bor 0x40000000L)
    $ku = [IntPtr]([long]1 -bor ([long]$sc -shl 16) -bor 0xC0000000L)
    [void][KeyProbeWin]::PostMessage($h, 0x0100, [IntPtr]$vk, $kd)
    [void][KeyProbeWin]::PostMessage($h, 0x0101, [IntPtr]$vk, $ku)
}
Start-Sleep -Milliseconds 1800
Shot $h $shotAfter
$brightAfter = MeanBrightness $shotAfter

# Log reading: windui traces one "key <Key> ..." line per KEYDOWN dispatch.
Start-Sleep -Milliseconds 500
$logs = Get-ChildItem -Path (Join-Path $root 'temp') -Filter 'rupix-*.log' -ErrorAction SilentlyContinue | Sort-Object Name
$keydownLines = @()
if ($logs) {
    $keydownLines = Select-String -Path $logs[0].FullName -Pattern ' key ' -ErrorAction SilentlyContinue
}
Write-Output ("RESULT key='{0}' charDirect={1} before={2} after={3} keydown_log_lines={4}" -f `
    $Key, [int]$SendCharDirect.IsPresent, $brightBefore, $brightAfter, @($keydownLines).Count)
foreach ($l in $keydownLines) { Write-Output ("KEYLOG " + $l.Line.Trim()) }

[void][KeyProbeWin]::PostMessage($h, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero)
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
