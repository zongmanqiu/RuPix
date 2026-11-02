# stall_probe.ps1 - sample how long the UI thread stops answering on its own,
# with no key pressed: opening an image, the native (full-size) level landing, paging.
#
# Why: rotate/mirror were the loudest stalls and are fixed (m72). Anything else
# that copies pixels on the UI thread shows up the same way, so measure the whole
# timeline instead of one key. Oracle = SendMessageTimeout(hwnd, WM_NULL, ...,
# SMTO_ABORTIFHUNG): a sent message is serviced only while the UI thread pumps,
# so one round trip IS the time the thread was busy (baseline ~0.1 ms).
# GetWindowRect/PrintWindow are not oracles (rect may stay put; PrintWindow can be
# served from a cached surface).
#
# Rules: ASCII only; private root + hardlinked exe + pinned ini + private temp\;
# only the child we started is closed (WM_CLOSE, own-pid fallback).
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File build\stall_probe.ps1 -Tag open -Seconds 10
#   powershell -ExecutionPolicy Bypass -File build\stall_probe.ps1 -Tag at100 -Seconds 20 -At100AfterMs 3000
#   powershell -ExecutionPolicy Bypass -File build\stall_probe.ps1 -Tag page -Page 5 -PageEveryMs 1600
# (-Page presses RIGHT on a fixed cadence, -At100AfterMs presses '1'; both fire from
#  inside the same sample loop, so one timeline covers open + zoom + paging.)
#
# -CopyAfterMs presses the copy hotkey (F9, bound only in this probe ini) that many ms
# in; -HoldClipboard keeps the clipboard OPEN from this thread while sampling, so the
# product's copy attempt is refused at OpenClipboard and the user's real clipboard is
# never overwritten. The encode cost is what stalls the UI thread, and it happens
# either way, so the denial is honest: it only changes the last millisecond.
# -TimeoutMs raises the WM_NULL timeout for arms that stall longer than 5 s.
# -ShotAtMs is a semicolon list of offsets at which to grab the window (toast text).

param(
    [Parameter(Position = 0)][string]$Tag = 'stall',
    [Parameter(Position = 1)][string]$Image = 'E:\tmp\AI.Agent\RuPix\.temp\big-samples\tiff\100M\tiff_100M.tif',
    [int]$Seconds = 8,
    [int]$IntervalMs = 15,
    [int]$Page = 0,
    [int]$PageEveryMs = 700,
    [int]$At100AfterMs = 0,
    [int]$CopyAfterMs = 0,
    [int]$TimeoutMs = 5000,
    [int]$HoldClipboard = 0,
    [string]$ShotAtMs = '',
    [int]$Prefetch = 0,
    [string]$ShotAfter = '',
    [switch]$DecodeTrace
)
$ErrorActionPreference = 'Stop'

$build = 'E:\tmp\AI.Agent\RuPix\.build'
$srcExe = Join-Path $build 'rupix.exe'
if (-not (Test-Path $srcExe)) { Write-Output 'NO_EXE'; exit 1 }
if (-not (Test-Path $Image)) { Write-Output 'NO_IMAGE'; exit 1 }

$root = "E:\tmp\AI.Agent\RuPix\.temp\probe-stall-$Tag"
New-Item -ItemType Directory -Force -Path $root | Out-Null
$exe = Join-Path $root 'rupix.exe'
Remove-Item -Force $exe -ErrorAction SilentlyContinue
try {
    New-Item -ItemType HardLink -Path $exe -Target $srcExe -ErrorAction Stop | Out-Null
} catch {
    Copy-Item $srcExe $exe -Force
}
Remove-Item -Recurse -Force (Join-Path $root 'temp') -ErrorAction SilentlyContinue

# RuPix is single-instance: a leftover child from an earlier arm of THIS rig makes a
# new launch exit instantly with no window. Sweep only processes whose image lives
# under .temp\probe-* (never the shipped .build copy the user may be running).
function Close-OwnProbe {
    $stray = Get-CimInstance Win32_Process -Filter "Name='rupix.exe'" |
        Where-Object { $_.ExecutablePath -like 'E:\tmp\AI.Agent\RuPix\.temp\probe-*' }
    foreach ($q in $stray) {
        $proc = Get-Process -Id $q.ProcessId -ErrorAction SilentlyContinue
        if ($proc -and $proc.MainWindowHandle -ne 0) {
            [void][StallWin]::PostMessage([IntPtr]$proc.MainWindowHandle, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero)
        }
    }
    for ($i = 0; $i -lt 20; $i++) {
        Start-Sleep -Milliseconds 100
        $left = $stray | Where-Object { Get-Process -Id $_.ProcessId -ErrorAction SilentlyContinue }
        if (-not $left) { break }
    }
    foreach ($q in ($stray | Where-Object { Get-Process -Id $_.ProcessId -ErrorAction SilentlyContinue })) {
        Stop-Process -Id $q.ProcessId -Force
        Write-Output ("SWEPT pid={0}" -f $q.ProcessId)
    }
}
@(
    "prefetch=$Prefetch", 'close_to_tray=0', 'minimal_mode=0',
    'decoders_preferred=', 'decoders_disabled=',
    'window_w=800', 'window_h=600', 'init_window=0',
    'opacity=100', 'filmstrip=0', 'checker=0', 'stay_on_top=0',
    # F9 is bound to the copy-image action only in this probe-private ini (factory: no key).
    'hotkeys=copy_img:F9'
) | Set-Content -Path (Join-Path $root 'rupix.ini') -Encoding ascii

$env:RUPIX_DECODERS_DIR = Join-Path $build 'decoders'
Remove-Item Env:RUST_LOG -ErrorAction SilentlyContinue
Remove-Item Env:RUPIX_DECODE_TRACE -ErrorAction SilentlyContinue
# -DecodeTrace is an opt-in *instrumented arm*: it re-arms the two vars the line above
# clears, so the same run that stamps [main-latency] cost= also yields the five-segment
# [decode-trace] split (read/probe/query/render/post). Default arms keep the shipped filter,
# so an arm with the switch is a different arm and must be tagged as such.
if ($DecodeTrace) {
    $env:RUST_LOG = 'debug,fontdb=error'
    $env:RUPIX_DECODE_TRACE = '1'
}

Add-Type -AssemblyName System.Drawing
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class StallWin {
    [StructLayout(LayoutKind.Sequential)]
    public struct RECT { public int Left, Top, Right, Bottom; }
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint msg, IntPtr w, IntPtr l);
    // Holding the clipboard open (WITHOUT EmptyClipboard: that would throw away whatever
    // the user has on it) blocks every other OpenClipboard, so the product's copy is
    // refused at the door and the user's clipboard survives the probe untouched.
    [DllImport("user32.dll")] public static extern bool OpenClipboard(IntPtr h);
    [DllImport("user32.dll")] public static extern bool CloseClipboard();
    [DllImport("user32.dll")] public static extern uint MapVirtualKeyW(uint vk, uint mapType);
    [DllImport("user32.dll", SetLastError = true)]
    public static extern IntPtr SendMessageTimeout(IntPtr h, uint msg, IntPtr w, IntPtr l, uint flags, uint timeout, out IntPtr result);
}
'@

function Shot($h, $path) {
    $r = New-Object StallWin+RECT
    [void][StallWin]::GetWindowRect($h, [ref]$r)
    $bmp = New-Object System.Drawing.Bitmap ($r.Right - $r.Left), ($r.Bottom - $r.Top)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $hdc = $g.GetHdc()
    [void][StallWin]::PrintWindow($h, $hdc, 2)
    $g.ReleaseHdc($hdc)
    $g.Dispose()
    $bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
    $bmp.Dispose()
    Write-Output ("SHOT {0}" -f $path)
}

function Press($h, [int]$vk) {
    $sc = ([int][StallWin]::MapVirtualKeyW([uint32]$vk, 1)) -band 0xffff
    $kd = [IntPtr]([long]1 -bor ([long]$sc -shl 16) -bor 0x40000000L)
    $ku = [IntPtr]([long]1 -bor ([long]$sc -shl 16) -bor 0xC0000000L)
    [void][StallWin]::PostMessage($h, 0x0100, [IntPtr]$vk, $kd)
    [void][StallWin]::PostMessage($h, 0x0101, [IntPtr]$vk, $ku)
}

Close-OwnProbe
if ($HoldClipboard -ne 0) {
    # Open, never Empty: the user's clipboard content stays exactly as it was.
    if ([StallWin]::OpenClipboard([IntPtr]::Zero)) {
        Write-Output 'CLIP_HELD=1 (product copy will be refused, clipboard untouched)'
    } else {
        Write-Output 'CLIP_HELD=0 (something else already holds it: readings still valid)'
    }
}
$p = Start-Process -FilePath $exe -ArgumentList "`"$Image`"" -PassThru -WorkingDirectory $root
$h = [IntPtr]::Zero
for ($i = 0; $i -lt 200; $i++) {
    Start-Sleep -Milliseconds 50
    $p.Refresh()
    if ($p.HasExited) { Write-Output 'ABORT_EXITED (another instance already running?)'; exit 1 }
    $h = [IntPtr]$p.MainWindowHandle
    if ($h -ne [IntPtr]::Zero) { break }
}
if ($h -eq [IntPtr]::Zero) { Write-Output 'ABORT_NO_WINDOW'; exit 1 }

$pressAt = New-Object 'System.Collections.Generic.List[int]'
$pressVk = New-Object 'System.Collections.Generic.List[int]'
for ($i = 1; $i -le $Page; $i++) {
    $pressAt.Add($PageEveryMs * $i)
    $pressVk.Add(0x27)
}
if ($At100AfterMs -gt 0) {
    $pressAt.Add($At100AfterMs)
    $pressVk.Add(0x31)
}
if ($CopyAfterMs -gt 0) {
    # F9 = copy current image; bound only in this probe-private ini.
    $pressAt.Add($CopyAfterMs)
    $pressVk.Add(0x78)
}
$shotAt = New-Object 'System.Collections.Generic.List[int]'
foreach ($s in $ShotAtMs.Split(';', [System.StringSplitOptions]::RemoveEmptyEntries)) {
    $shotAt.Add([int]$s.Trim())
}
$shotFired = New-Object 'System.Collections.Generic.List[bool]'
for ($i = 0; $i -lt $shotAt.Count; $i++) { $shotFired.Add($false) }

# t0 = window is up; sampling starts immediately so the first decode and the
# native-level install both fall inside the timeline. The presses above fire on
# schedule inside the same sample loop, so open + 100% + paging read out of one
# paired timeline.
$sw = [System.Diagnostics.Stopwatch]::StartNew()
$rows = @()
$fired = New-Object 'System.Collections.Generic.List[bool]'
for ($i = 0; $i -lt $pressAt.Count; $i++) { $fired.Add($false) }
$samples = [int]($Seconds * 1000 / $IntervalMs)
for ($n = 0; $n -lt $samples; $n++) {
    $target = $n * $IntervalMs
    while ($sw.Elapsed.TotalMilliseconds -lt $target) { Start-Sleep -Milliseconds 1 }
    for ($i = 0; $i -lt $pressAt.Count; $i++) {
        if (-not $fired[$i] -and $sw.Elapsed.TotalMilliseconds -ge $pressAt[$i]) {
            Press $h $pressVk[$i]
            # Wall clock too: the app log stamps [HH:MM:SS.mmm], so "key pressed -> the
            # next image landed" can only be computed by joining the two clocks.
            Write-Output ("PRESS at_ms={0:N0} vk={1} wall={2}" -f $sw.Elapsed.TotalMilliseconds, $pressVk[$i], (Get-Date -Format 'HH:mm:ss.fff'))
            $fired[$i] = $true
        }
    }
    for ($i = 0; $i -lt $shotAt.Count; $i++) {
        if (-not $shotFired[$i] -and $sw.Elapsed.TotalMilliseconds -ge $shotAt[$i]) {
            $shotFired[$i] = $true
            Shot $h (Join-Path $root "$Tag-shot$($shotAt[$i]).png")
        }
    }
    $at = $sw.Elapsed.TotalMilliseconds
    $res = [IntPtr]::Zero
    $ok = [StallWin]::SendMessageTimeout($h, 0x0000, [IntPtr]::Zero, [IntPtr]::Zero, 0x0002, $TimeoutMs, [ref]$res)
    $rt = $sw.Elapsed.TotalMilliseconds - $at
    $rows += ,@($at, $rt, ($ok -ne [IntPtr]::Zero))
}

if ($HoldClipboard -ne 0) {
    [void][StallWin]::CloseClipboard()
    Write-Output 'CLIP_RELEASED'
}

$worst = $rows | Sort-Object { $_[1] } -Descending | Select-Object -First 6
$sum = ($rows | ForEach-Object { $_[1] } | Measure-Object -Sum).Sum
$over20 = @($rows | Where-Object { $_[1] -gt 20 }).Count
$dead = @($rows | Where-Object { $_[2] -eq $false }).Count
Write-Output ("STALL tag={0} image={1} samples={2} blocked_total_ms={3:N1} over20={4} timeout={5} wminull_timeout_ms={6}" -f `
        $Tag, (Split-Path $Image -Leaf), $rows.Count, $sum, $over20, $dead, $TimeoutMs)
foreach ($w in $worst) {
    Write-Output ("  stall at_ms={0:N0} roundtrip_ms={1:N1} ok={2}" -f $w[0], $w[1], $w[2])
}

if ($ShotAfter -ne '') { Shot $h $ShotAfter }

Write-Output '--- log tail ---'
$logs = Get-ChildItem -Path (Join-Path $root 'temp') -Filter 'rupix-*.log' -ErrorAction SilentlyContinue
if ($logs) {
    if ($DecodeTrace) {
        # With the debug filter on, a 14-line tail is mostly other noise: pick the two
        # machine-readable lines this arm exists for.
        Select-String -Path $logs[0].FullName -Pattern 'decode-trace|main-latency' -Encoding UTF8 |
            Select-Object -Last 12 | ForEach-Object { Write-Output $_.Line }
    } else {
        Get-Content -Path $logs[0].FullName -Tail 14 -Encoding UTF8
    }
} else { Write-Output 'NO_LOG' }

[void][StallWin]::PostMessage($h, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero)
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
