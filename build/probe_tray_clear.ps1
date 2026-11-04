# RuPix probe: "close to tray, wake it back -> the old image must be gone"
#
# Why this probe exists (K189, 2026-11-04)
# ----------------------------------------
# The window state used to be pinned by a stray `let first = WinRt::create(..)` binding
# in `runtime::run`. The OS window went away, the state did not, so `Shared::newest()`
# still found "the window the user just closed"; the tray then re-opened THAT window and
# the old picture (plus its filmstrip) came back. Symptom reported by the user:
# "close to tray and open again, the image is not cleared".
#
# What we assert here:
#   1. after a close, waking without a file gives an EMPTY window (screenshot is small);
#   2. waking WITH a file gives a window that has an image (screenshot is big);
#   3. neither working-set nor thread count grows across rounds (state really died);
#   4. with allow_multiple=1: closing one of two windows leaves the other one intact,
#      closing the last one keeps the process in the tray, wake gives an empty window.
#
# Usage: powershell -ExecutionPolicy Bypass -File build\probe_tray_clear.ps1 [-Rounds 4]
# Exit:  0 = PASS, 1 = FAIL.
#
# NOTE: this file is ASCII-only on purpose. Windows PowerShell 5.1 reads a BOM-less
# script as ANSI; non-ASCII comments get mangled and can swallow the next line (K27).

param(
    [int]$Rounds = 4
)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Text;
using System.Collections;
using System.Runtime.InteropServices;
public class RuPixProbe {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr l);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern int GetClassName(IntPtr h, StringBuilder sb, int n);
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint msg, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder sb, int n);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr dc, uint flags);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
}
"@

# build\ lives under main\, while .build\ (deploy output) is one level above main\.
$main = Split-Path -Parent $PSScriptRoot
$repo = Split-Path -Parent $main
$build = Join-Path $repo '.build'
if (-not (Test-Path (Join-Path $build 'rupix.exe'))) {
    $build = Join-Path $main '.build'
}
if (-not (Test-Path (Join-Path $build 'rupix.exe'))) {
    Write-Output ("FAIL: no rupix.exe under " + (Join-Path $repo '.build'))
    exit 1
}

$problems = New-Object Collections.ArrayList
function Fail($msg) {
    [void]$script:problems.Add($msg)
    Write-Output ("  !! " + $msg)
}

# The single-instance mutex is global (same APP_ID): leftover processes would forward
# argv and exit, and every reading below would belong to the old process.
$ErrorActionPreference = 'Continue'
& taskkill.exe /F /IM rupix.exe | Out-Null
$ErrorActionPreference = 'Stop'
Start-Sleep -Milliseconds 900

$stamp = [DateTime]::Now.ToString('HHmmss')
$root = Join-Path $env:TEMP ('rupix_probe_tray_' + $stamp)
New-Item -ItemType Directory -Path $root | Out-Null
Copy-Item (Join-Path $build 'rupix.exe') $root
Copy-Item (Join-Path $build 'decoders') $root -Recurse
Write-Output ("root " + $root)

# Kill every instance and clear the "was running" marker.
#
# The marker matters: a hard kill looks like a crash to the next launch, which then pops
# the "RuPix last exited abnormally" window. That is correct app behaviour, but it is a
# second WindUiWindowClass window and would be mistaken for a viewer window here.
function Stop-All {
    $ErrorActionPreference = 'Continue'
    & taskkill.exe /F /IM rupix.exe | Out-Null
    $ErrorActionPreference = 'Stop'
    Start-Sleep -Milliseconds 900
    foreach ($m in @('rupix.run')) {
        $f = Join-Path (Join-Path $script:root 'temp') $m
        if (Test-Path $f) {
            try { [IO.File]::Delete($f) } catch { }
        }
    }
    $hint = Join-Path $script:root 'temp'
    if (Test-Path $hint) {
        Get-ChildItem $hint -Filter '*.dmp' -ErrorAction SilentlyContinue | ForEach-Object {
            try { [IO.File]::Delete($_.FullName) } catch { }
        }
    }
}

function Write-Ini([string]$text) {
    [IO.File]::WriteAllText((Join-Path $script:root 'rupix.ini'), $text, (New-Object Text.UTF8Encoding($false)))
}

function ViewerWindows($procId) {
    $script:found = New-Object Collections.ArrayList
    $cb = [RuPixProbe+EnumProc]{
        param($h, $l)
        $p = 0
        [RuPixProbe]::GetWindowThreadProcessId($h, [ref]$p) | Out-Null
        if ($p -eq $procId) {
            $sb = New-Object Text.StringBuilder 256
            [RuPixProbe]::GetClassName($h, $sb, 256) | Out-Null
            if ($sb.ToString() -eq 'WindUiWindowClass' -and [RuPixProbe]::IsWindowVisible($h)) {
                [void]$script:found.Add($h)
            }
        }
        return $true
    }
    [RuPixProbe]::EnumWindows($cb, [IntPtr]::Zero) | Out-Null
    return $script:found
}

function ShotSize($hwnd, $path) {
    if ($hwnd -eq [IntPtr]::Zero) { return 0 }
    $r = New-Object RuPixProbe+RECT
    [RuPixProbe]::GetWindowRect($hwnd, [ref]$r) | Out-Null
    $w = $r.R - $r.L
    $h = $r.B - $r.T
    if ($w -le 0 -or $h -le 0) { return 0 }
    $bmp = New-Object Drawing.Bitmap $w, $h
    $g = [Drawing.Graphics]::FromImage($bmp)
    $dc = $g.GetHdc()
    [RuPixProbe]::PrintWindow($hwnd, $dc, 2) | Out-Null
    $g.ReleaseHdc($dc)
    $g.Dispose()
    $bmp.Save($path, [Drawing.Imaging.ImageFormat]::Png)
    $bmp.Dispose()
    return (Get-Item $path).Length
}

function Close-Window($hwnd) {
    if ($hwnd -eq [IntPtr]::Zero) { return }
    [RuPixProbe]::PostMessage($hwnd, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null
    Start-Sleep -Milliseconds 800
}

# An empty viewer window compresses to a few KB; one showing a photo cannot.
$EMPTY_MAX = 60000

$exe = Join-Path $root 'rupix.exe'
$imgs = @(Get-ChildItem (Join-Path $build '1M') -Filter '*.jpg' | Select-Object -First 2)
if ($imgs.Count -eq 0) {
    $imgs = @(Get-ChildItem (Join-Path $build '1M') -Filter '*.png' | Select-Object -First 2)
}
$img1 = $imgs[0].FullName
$img2 = if ($imgs.Count -gt 1) { $imgs[1].FullName } else { $img1 }

# ---------------------------------------------------------------- single window rounds
Write-Output "== part 1: single window, close/wake rounds"
Write-Ini "close_to_tray=1`nallow_multiple=0`n"
$p = Start-Process -FilePath $exe -ArgumentList "`"$img1`"" -PassThru
Start-Sleep -Seconds 4
$p = Get-Process -Id $p.Id -ErrorAction SilentlyContinue
if (-not $p) { Write-Output "FAIL: first instance did not stay alive"; exit 1 }

$thrClosedFirst = -1
$memClosedFirst = -1
for ($i = 1; $i -le $Rounds; $i++) {
    $wins = ViewerWindows $p.Id
    if ($wins.Count -ne 1) { Fail ("round {0}: expected 1 window before close, got {1}" -f $i, $wins.Count) }
    Close-Window ($wins | Select-Object -First 1)

    $procNow = Get-Process -Id $p.Id -ErrorAction SilentlyContinue
    if (-not $procNow) { Write-Output "FAIL: process died on close (close_to_tray=1 must keep it)"; exit 1 }
    $after = ViewerWindows $p.Id
    if ($after.Count -ne 0) { Fail ("round {0}: window still visible after close" -f $i) }
    if ($i -eq 1) { $thrClosedFirst = $procNow.Threads.Count; $memClosedFirst = [math]::Round($procNow.WorkingSet64 / 1MB, 1) }
    if ($i -eq $Rounds) {
        $thrLast = $procNow.Threads.Count
        $memLast = [math]::Round($procNow.WorkingSet64 / 1MB, 1)
        # Threads must not accumulate: one leaked decoder thread per close would show up here.
        if ($thrLast -gt $thrClosedFirst + 1) {
            Fail ("threads grew while closed: {0} -> {1}" -f $thrClosedFirst, $thrLast)
        }
        if ($memLast -gt $memClosedFirst + 8) {
            Fail ("working set grew while closed: {0}MB -> {1}MB" -f $memClosedFirst, $memLast)
        }
    }

    $img = if ($i % 2 -eq 0) { $img2 } else { $img1 }
    $p2 = Start-Process -FilePath $exe -ArgumentList "`"$img`"" -PassThru
    Start-Sleep -Seconds 2
    if ($p2 -and -not $p2.HasExited) { Stop-Process -Id $p2.Id -Force -ErrorAction SilentlyContinue }

    $wins = ViewerWindows $p.Id
    if ($wins.Count -ne 1) { Fail ("round {0}: after waking with a file expected 1 window, got {1}" -f $i, $wins.Count) }
    $png = ShotSize ($wins | Select-Object -First 1) (Join-Path $root ("round{0}.png" -f $i))
    if ($png -le $EMPTY_MAX) { Fail ("round {0}: window looks empty after opening a file (png={1})" -f $i, $png) }
    Write-Output ("  round {0}: closed(mem={1}MB thr={2}) -> reopened png={3}" -f $i, $memClosedFirst, $thrClosedFirst, $png)
}

# --------------------------------------------------- wake without a file => empty window
Write-Output "== part 2: close, then wake without a file (the reported bug)"
Close-Window ((ViewerWindows $p.Id) | Select-Object -First 1)
$p3 = Start-Process -FilePath $exe -PassThru
Start-Sleep -Seconds 2
$wins = ViewerWindows $p.Id
$png = ShotSize ($wins | Select-Object -First 1) (Join-Path $root 'wake_empty.png')
if ($wins.Count -ne 1) { Fail ("wake: expected 1 window, got {0}" -f $wins.Count) }
if ($png -ge $EMPTY_MAX) { Fail ("wake: the window still shows the old picture (png={0})" -f $png) }
Write-Output ("  wake png={0} (empty window)" -f $png)
if ($p3 -and -not $p3.HasExited) { Stop-Process -Id $p3.Id -Force -ErrorAction SilentlyContinue }
Stop-All

# ------------------------------------------------------------- two windows side by side
Write-Output "== part 3: allow_multiple=1, two windows"
Write-Ini "close_to_tray=1`nallow_multiple=1`n"
$p = Start-Process -FilePath $exe -ArgumentList "`"$img1`"" -PassThru
Start-Sleep -Seconds 3
$p2 = Start-Process -FilePath $exe -ArgumentList "`"$img2`"" -PassThru
Start-Sleep -Seconds 3
$wins = ViewerWindows $p.Id
if ($wins.Count -ne 2) { Fail ("multi: expected 2 windows in one process, got {0}" -f $wins.Count) }
if ($p2 -and -not $p2.HasExited) { Fail "multi: second instance stayed alive (must hand off)" }

if ($wins.Count -ge 2) {
    Close-Window $wins[0]
    $left = ViewerWindows $p.Id
    if ($left.Count -ne 1) { Fail ("multi: after closing one expected 1 window, got {0}" -f $left.Count) }
    $png = ShotSize ($left | Select-Object -First 1) (Join-Path $root 'multi_left.png')
    if ($png -le $EMPTY_MAX) { Fail ("multi: the surviving window lost its picture (png={0})" -f $png) }
    Write-Output ("  one closed, the other keeps its picture (png={0})" -f $png)

    Close-Window ($left | Select-Object -First 1)
    $alive = [bool](Get-Process -Id $p.Id -ErrorAction SilentlyContinue)
    if (-not $alive) { Fail "multi: process exited after the last window (close_to_tray=1 must keep it)" }
    $p4 = Start-Process -FilePath $exe -PassThru
    Start-Sleep -Seconds 2
    $wake = ViewerWindows $p.Id
    $png = ShotSize ($wake | Select-Object -First 1) (Join-Path $root 'multi_wake.png')
    if ($wake.Count -ne 1) { Fail ("multi: wake expected 1 window, got {0}" -f $wake.Count) }
    if ($png -ge $EMPTY_MAX) { Fail ("multi: woken window still shows a picture (png={0})" -f $png) }
    Write-Output ("  all closed -> process alive, wake png={0}" -f $png)
    if ($p4 -and -not $p4.HasExited) { Stop-Process -Id $p4.Id -Force -ErrorAction SilentlyContinue }
}

Stop-All

if ($problems.Count -eq 0) {
    Write-Output "PROBE PASS"
    exit 0
}
Write-Output ("PROBE FAIL: " + $problems.Count + " problem(s)")
exit 1
