# rotate_probe.ps1 - measure how long the UI thread stops answering when the
# viewer rotates / mirrors a large image (R / M), on the shipped exe.
#
# Why: `Viewer::rotate_cw` / `toggle_flip` rebuild the on-screen bitmap on the UI
# thread. At the fit level the bitmap is viewport-sized, but once the viewer holds
# the native (full-size) level the same rebuild copies every pixel: 36.7 MP measured
# 170-190 ms, 100 MP 540-600 ms inside `geometry::transformed`.
#
# Oracle: SendMessageTimeout(hwnd, WM_NULL, ...) - a busy UI thread cannot answer
# it. We sample it around one R press and report the longest unbroken silence.
# (GetWindowRect/PrintWindow are NOT the oracle here: the rect does not change and
# PrintWindow an be served from a cached surface.)
#
# Rules: ASCII only; private root + hardlinked exe + pinned ini + private temp\; only
# the child we started is closed (WM_CLOSE, own-pid fallback).
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File build\rotate_probe.ps1 -Tag pre
#   powershell -ExecutionPolicy Bypass -File build\rotate_probe.ps1 -Tag post -Key M

param(
    [Parameter(Position = 0)][string]$Tag = 'rot',
    [Parameter(Position = 1)][string]$Image = 'E:\tmp\AI.Agent\RuPix\.temp\big-samples\tiff\100M\tiff_100M.tif',
    [string]$Key = 'R',
    [switch]$Zoom100,
    [switch]$BackToFit,
    [int]$Prefetch = 0,
    [string]$SecondKey = '',
    [int]$SecondDelayMs = 600,
    [int]$SettleSeconds = 10,
    [int]$MeasureMs = 2000,
    [string]$ShotBefore = '',
    [string]$ShotAfter = ''
)
$ErrorActionPreference = 'Stop'

$build = 'E:\tmp\AI.Agent\RuPix\.build'
$srcExe = Join-Path $build 'rupix.exe'
if (-not (Test-Path $srcExe)) { Write-Output 'NO_EXE'; exit 1 }
if (-not (Test-Path $Image)) { Write-Output 'NO_IMAGE'; exit 1 }

$root = "E:\tmp\AI.Agent\RuPix\.temp\probe-rot-$Tag"
New-Item -ItemType Directory -Force -Path $root | Out-Null
$exe = Join-Path $root 'rupix.exe'
Remove-Item -Force $exe -ErrorAction SilentlyContinue
try {
    New-Item -ItemType HardLink -Path $exe -Target $srcExe -ErrorAction Stop | Out-Null
} catch {
    Copy-Item $srcExe $exe -Force
}
Remove-Item -Recurse -Force (Join-Path $root 'temp') -ErrorAction SilentlyContinue
@(
    "prefetch=$Prefetch", 'close_to_tray=0', 'minimal_mode=0',
    'decoders_preferred=', 'decoders_disabled=',
    'window_w=800', 'window_h=600', 'init_window=0',
    'opacity=100', 'filmstrip=0', 'checker=0', 'stay_on_top=0'
) | Set-Content -Path (Join-Path $root 'rupix.ini') -Encoding ascii

$env:RUPIX_DECODERS_DIR = Join-Path $build 'decoders'
Remove-Item Env:RUST_LOG -ErrorAction SilentlyContinue
Remove-Item Env:RUPIX_DECODE_TRACE -ErrorAction SilentlyContinue

Add-Type -AssemblyName System.Drawing
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class RotProbeWin {
    [StructLayout(LayoutKind.Sequential)]
    public struct RECT { public int Left, Top, Right, Bottom; }
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint msg, IntPtr w, IntPtr l);
    [DllImport("user32.dll")] public static extern uint MapVirtualKeyW(uint vk, uint mapType);
    [DllImport("user32.dll", SetLastError = true)]
    public static extern IntPtr SendMessageTimeout(IntPtr h, uint msg, IntPtr w, IntPtr l, uint flags, uint timeout, out IntPtr result);
}
'@

function Shot($h, $path) {
    $r = New-Object RotProbeWin+RECT
    [void][RotProbeWin]::GetWindowRect($h, [ref]$r)
    $bmp = New-Object System.Drawing.Bitmap ($r.Right - $r.Left), ($r.Bottom - $r.Top)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $hdc = $g.GetHdc()
    [void][RotProbeWin]::PrintWindow($h, $hdc, 2)
    $g.ReleaseHdc($hdc)
    $g.Dispose()
    $bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
    $bmp.Dispose()
    Write-Output ("SHOT {0}" -f $path)
}

function Press($h, [int]$vk) {
    $sc = ([int][RotProbeWin]::MapVirtualKeyW([uint32]$vk, 1)) -band 0xffff
    $kd = [IntPtr]([long]1 -bor ([long]$sc -shl 16) -bor 0x40000000L)
    $ku = [IntPtr]([long]1 -bor ([long]$sc -shl 16) -bor 0xC0000000L)
    [void][RotProbeWin]::PostMessage($h, 0x0100, [IntPtr]$vk, $kd)
    [void][RotProbeWin]::PostMessage($h, 0x0101, [IntPtr]$vk, $ku)
}

$p = Start-Process -FilePath $exe -ArgumentList "`"$Image`"" -PassThru -WorkingDirectory $root
$h = [IntPtr]::Zero
for ($i = 0; $i -lt 100; $i++) {
    Start-Sleep -Milliseconds 100
    $p.Refresh()
    if ($p.HasExited) { Write-Output 'ABORT_EXITED (another instance already running?)'; exit 1 }
    $h = [IntPtr]$p.MainWindowHandle
    if ($h -ne [IntPtr]::Zero) { break }
}
if ($h -eq [IntPtr]::Zero) { Write-Output 'ABORT_NO_WINDOW'; exit 1 }
Write-Output ("WINDOW pid={0} hwnd={1}" -f $p.Id, $h)

Start-Sleep -Seconds $SettleSeconds
if ($Zoom100) {
    Press $h 0x31   # '1' = actual size / 100%
    Start-Sleep -Milliseconds 2500
    if ($BackToFit) {
        Press $h 0x30   # '0' = fit to window
        Start-Sleep -Milliseconds 1500
    }
}
if ($ShotBefore -ne '') { Shot $h $ShotBefore }

# Round-trip oracle: a sent WM_NULL is serviced by the UI thread only when it is
# pumping messages. Send it right after the key has been picked up (20 ms) and the
# call returns only when the handler has finished. Baseline arms prove the oracle
# itself is ~0 when nothing is being rebuilt.
$res = [IntPtr]::Zero
$base = @()
for ($i = 0; $i -lt 3; $i++) {
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $ok = [RotProbeWin]::SendMessageTimeout($h, 0x0000, [IntPtr]::Zero, [IntPtr]::Zero, 0x0002, 5000, [ref]$res)
    $base += $sw.Elapsed.TotalMilliseconds
}
Write-Output ("BASE roundtrip_ms=" + (($base | ForEach-Object { "{0:N1}" -f $_ }) -join ','))

$vk = [int][char]$Key
Press $h $vk
Start-Sleep -Milliseconds 20
$sw = [System.Diagnostics.Stopwatch]::StartNew()
$ok = [RotProbeWin]::SendMessageTimeout($h, 0x0000, [IntPtr]::Zero, [IntPtr]::Zero, 0x0002, 5000, [ref]$res)
$rt = $sw.Elapsed.TotalMilliseconds
Write-Output ("RESULT tag={0} key={1} zoom100={2} ui_roundtrip_ms={3:N1} ok={4}" -f $Tag, $Key, [int]$Zoom100.IsPresent, $rt, ($ok -ne [IntPtr]::Zero))

if ($SecondKey -ne '') {
    Start-Sleep -Milliseconds $SecondDelayMs
    $vk2 = if ($SecondKey -eq 'RIGHT') { 0x27 } elseif ($SecondKey -eq 'LEFT') { 0x25 } else { [int][char]$SecondKey }
    Press $h $vk2
    Start-Sleep -Milliseconds 20
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $ok = [RotProbeWin]::SendMessageTimeout($h, 0x0000, [IntPtr]::Zero, [IntPtr]::Zero, 0x0002, 5000, [ref]$res)
    $rt2 = $sw.Elapsed.TotalMilliseconds
    Write-Output ("RESULT2 tag={0} key={1} ui_roundtrip_ms={2:N1} ok={3}" -f $Tag, $SecondKey, $rt2, ($ok -ne [IntPtr]::Zero))
}

if ($ShotAfter -ne '') {
    Start-Sleep -Milliseconds 1500
    Shot $h $ShotAfter
}

Write-Output '--- log tail ---'
$logs = Get-ChildItem -Path (Join-Path $root 'temp') -Filter 'rupix-*.log' -ErrorAction SilentlyContinue
if ($logs) { Get-Content -Path $logs[0].FullName -Tail 12 -Encoding UTF8 } else { Write-Output 'NO_LOG' }

[void][RotProbeWin]::PostMessage($h, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero)
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
