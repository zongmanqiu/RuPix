# ============================================================================
# startup_probe.ps1 - measure what the user waits for BEFORE the first frame.
#
# The existing stall_probe starts its clock AFTER the window is up (its own
# comment says "t0 = window is up"), so the double-click -> window -> first
# picture span has never had a reading. This rig closes that gap with ONE clock
# source: a stopwatch started right before Start-Process, sampled in a poll loop
# that watches (a) the Win32 window oracle and (b) the product's own log lines.
#
# Marks, in the order they should appear for `rupix.exe <file>`:
#   spawn        the rig's own Start-Process cost (it is part of win, subtract it)
#   win        a top-level, visible window of THIS pid with a non-empty rect
#   log        the day-named run log exists and is readable
#   open       the "opened from command line" info line (only with a file arg)
#   plug=first/last  span of the "decoder loaded" info lines = the DLL scan
#              (last = tick at which the 12th such line first showed up)
#   frame      the first [main-latency] verdict line = decode finished; paint
#              follows within one poll tick
#   cost       that line's own cost= field (the app's clock, decode only)
#   ws         working set in MB read at the frame tick (occupancy leg)
#   wsSettled  working set re-read at -SettleWsMs after spawn. The frame tick is
#              NOT comparable across arms: they reach it ~300 ms apart, so "first
#              frame" samples two different moments of startup.
#   peak       PeakWorkingSet64 at that same offset. This is the honest occupancy
#              reading, because the product itself calls EmptyWorkingSet at 2 s and
#              10 s (runtime/mod.rs) - a later ws sample only shows that trim floor
#              (~0.6 MB, arm-independent), while a peak survives trimming and records
#              the whole startup including the background font warm.
#   pollGap    largest gap the loop itself observed = the honest resolution
#
# Resolution: Sleep(1) is not 1 ms on Windows, so a gap smaller than pollGap is
# not a difference. Polling reads the whole (small) log each tick and records
# when each line FIRST became visible.
#
# Never touches the user's clipboard; only ever signals processes it started or
# leftover children of .temp\probe-* (never the shipped .build copy).
# ============================================================================
param(
    [Parameter(Position = 0)][string]$Tag = 'startup',
    [Parameter(Position = 1)][string]$Image = '',
    [int]$HoldMs = 6000,
    [int]$SettleWsMs = 0,
    [string]$Exe = '',
    [switch]$TraceDebug,
    [string]$Ledger = ''
)
$ErrorActionPreference = 'Stop'

$build = 'E:\tmp\AI.Agent\RuPix\.build'
# -Exe points the rig at a different binary (an attribution arm with temporary
# stopwatch lines lives in .temp\target, never in .build). A different binary is
# a different arm and must be tagged as such.
$srcExe = if ($Exe -eq '') { Join-Path $build 'rupix.exe' } else { $Exe }
if (-not (Test-Path $srcExe)) { Write-Output 'NO_EXE'; exit 1 }
if ($Image -ne '' -and -not (Test-Path $Image)) { Write-Output 'NO_IMAGE'; exit 1 }

$root = "E:\tmp\AI.Agent\RuPix\.temp\probe-startup-$Tag"
New-Item -ItemType Directory -Force -Path $root | Out-Null
$exe = Join-Path $root 'rupix.exe'
Remove-Item -Force $exe -ErrorAction SilentlyContinue
try {
    New-Item -ItemType HardLink -Path $exe -Target $srcExe -ErrorAction Stop | Out-Null
} catch {
    Copy-Item $srcExe $exe -Force
}
Remove-Item -Recurse -Force (Join-Path $root 'temp') -ErrorAction SilentlyContinue

# Same pinned config as stall_probe so both rigs describe one product shape.
@(
    'close_to_tray=0', 'minimal_mode=0',
    'decoders_preferred=', 'decoders_disabled=',
    'window_w=800', 'window_h=600', 'init_window=0',
    'opacity=100', 'filmstrip=0', 'checker=0', 'stay_on_top=0'
) | Set-Content -Path (Join-Path $root 'rupix.ini') -Encoding ascii

$env:RUPIX_DECODERS_DIR = Join-Path $build 'decoders'
Remove-Item Env:RUST_LOG -ErrorAction SilentlyContinue
Remove-Item Env:RUPIX_DECODE_TRACE -ErrorAction SilentlyContinue
# -TraceDebug is an ATTRIBUTION arm, not a comparison arm: it lifts the filter to
# debug so the silent gap between "opened from command line" and the decoder scan
# gets witnesses. Any run with it must be tagged as a different arm.
if ($TraceDebug) { $env:RUST_LOG = 'debug,fontdb=error' }

Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class WinStartup {
    public delegate bool EnumProc(IntPtr h, IntPtr l);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr l);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint msg, IntPtr w, IntPtr l);
    public static IntPtr FindVisible(uint pid) {
        IntPtr found = IntPtr.Zero;
        EnumWindows(delegate (IntPtr h, IntPtr l) {
            uint p;
            GetWindowThreadProcessId(h, out p);
            if (p != pid || !IsWindowVisible(h)) return true;
            RECT r;
            if (!GetWindowRect(h, out r)) return true;
            if ((r.R - r.L) < 50 || (r.B - r.T) < 50) return true;
            found = h;
            return false;
        }, IntPtr.Zero);
        return found;
    }
}
'@

# A leftover child of THIS rig makes a new launch hand off and exit (single
# instance gate). WM_CLOSE first, force only if it ignores us.
$stray = @(Get-CimInstance Win32_Process -Filter "Name='rupix.exe'" |
    Where-Object { $_.ExecutablePath -like 'E:\tmp\AI.Agent\RuPix\.temp\probe-*' })
foreach ($q in $stray) {
    $proc = Get-Process -Id $q.ProcessId -ErrorAction SilentlyContinue
    if ($proc -and $proc.MainWindowHandle -ne 0) {
        [void][WinStartup]::PostMessage([IntPtr]$proc.MainWindowHandle, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero)
    }
}
foreach ($q in $stray) {
    for ($i = 0; $i -lt 20; $i++) {
        Start-Sleep -Milliseconds 100
        if (-not (Get-Process -Id $q.ProcessId -ErrorAction SilentlyContinue)) { break }
    }
    if (Get-Process -Id $q.ProcessId -ErrorAction SilentlyContinue) {
        Stop-Process -Id $q.ProcessId -Force
        Write-Output ("SWEPT pid={0}" -f $q.ProcessId)
    }
}

# Chinese needles spelled from code points so this script stays pure ASCII and
# the console code page cannot mangle it. They are deliberately longer than the
# obvious word: the session preamble also contains "command line", and the
# settings page logs about decoders too, so the match must be the full phrase of
# the two lines we actually want to time.
$needleOpen = [string][char]0x547D + [string][char]0x4EE4 + [string][char]0x884C + [string][char]0x6253 + [string][char]0x5F00   # ming-ling-hang-da-kai
$needlePlug = [string][char]0x5DF2 + [string][char]0x52A0 + [string][char]0x8F7D + [string][char]0x89E3 + [string][char]0x7801   # yi-jia-zai-jie-ma

$day = Get-Date -Format 'yyyy-MM-dd'
$logPath = Join-Path $root "temp\rupix-$day.log"

# Warm the three things the loop calls for the first time (P/Invoke marshal,
# FileStream + StreamReader, regex) BEFORE the clock starts; without this the
# first tick costs ~90 ms and that resolution is too coarse to tell 400 from 300.
$iniPath = Join-Path $root 'rupix.ini'
[void][WinStartup]::FindVisible([uint32]4294967295)
$wfs = [IO.File]::Open($iniPath, 'Open', 'Read', 'ReadWrite')
$wsr = New-Object IO.StreamReader($wfs, [Text.Encoding]::UTF8)
$warmTxt = $wsr.ReadToEnd()
$wsr.Dispose(); $wfs.Dispose()
[void]([regex]::Matches($warmTxt, [regex]::Escape('rupix'))).Count
[void]('cost=1.0' -match 'cost=([0-9.]+)')

# Start-Process refuses an empty ArgumentList array (validation error), so a
# blank-start arm passes no parameter at all.
if ($Image -eq '') {
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $p = Start-Process -FilePath $exe -PassThru -WorkingDirectory $root
} else {
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $p = Start-Process -FilePath $exe -ArgumentList "`"$Image`"" -PassThru -WorkingDirectory $root
}
# Start-Process itself is .NET interop + CreateProcess: it eats the first ~85 ms
# of the timeline before our loop even begins (that stall IS the loop's first
# pollGap). Record it so win can be read both ways instead of blaming the rig
# on the product.
$spawnMs = $sw.Elapsed.TotalMilliseconds
$thePid = $p.Id

$mark = [ordered]@{ win = -1.0; log = -1.0; open = -1.0; plugFirst = -1.0; plugLast = -1.0; frame = -1.0 }
$plugCount = 0
$costTxt = 'NONE'
$wsTxt = 'NONE'
$wsSettledTxt = if ($SettleWsMs -gt 0) { 'PENDING' } else { 'NONE' }
$peakTxt = 'NONE'
$hwnd = [IntPtr]::Zero
$prev = 0.0
$maxGap = 0.0

while ($sw.Elapsed.TotalMilliseconds -lt $HoldMs) {
    [Threading.Thread]::Sleep(1)
    $now = $sw.Elapsed.TotalMilliseconds
    if ($now - $prev -gt $maxGap) { $maxGap = $now - $prev }
    $prev = $now

    if ($mark.win -lt 0) {
        $h = [WinStartup]::FindVisible([uint32]$thePid)
        if ($h -ne [IntPtr]::Zero) { $mark.win = $now; $hwnd = $h }
    }
    if (-not (Test-Path $logPath)) { continue }
    if ($mark.log -lt 0) { $mark.log = $now }
    # (*) FileShare ReadWrite is load-bearing: the product holds this log open for
    # append, and ReadAllText's own default share mode denies that write, so every
    # read throws and the log-derived marks silently stay at -1.
    $txt = ''
    try {
        $fs = [IO.File]::Open($logPath, 'Open', 'Read', 'ReadWrite')
        try {
            $sr = New-Object IO.StreamReader($fs, [Text.Encoding]::UTF8)
            $txt = $sr.ReadToEnd()
            $sr.Dispose()
        } finally { $fs.Dispose() }
    } catch { continue }
    if ($mark.open -lt 0 -and $txt.Contains($needleOpen)) { $mark.open = $now }
    $c = ([regex]::Matches($txt, [regex]::Escape($needlePlug))).Count
    if ($c -gt $plugCount) {
        if ($plugCount -eq 0) { $mark.plugFirst = $now }
        $plugCount = $c
        $mark.plugLast = $now
    }
    $i = $txt.IndexOf('[main-latency]')
    if ($i -ge 0 -and $mark.frame -lt 0) {
        $mark.frame = $now
        $tail = $txt.Substring($i, [Math]::Min(200, $txt.Length - $i))
        $m = [regex]::Match($tail, 'cost=([0-9.]+)')
        if ($m.Success) { $costTxt = $m.Groups[1].Value }
        try {
            $pp = Get-Process -Id $thePid -ErrorAction Stop
            $wsTxt = ('{0:N1}' -f ($pp.WorkingSet64 / 1MB))
        } catch { }
    }
    $needSettle = ($SettleWsMs -gt 0 -and $wsSettledTxt -eq 'PENDING')
    if ($needSettle -and $now -ge $SettleWsMs) {
        try {
            $pp = Get-Process -Id $thePid -ErrorAction Stop
            $wsSettledTxt = ('{0:N1}' -f ($pp.WorkingSet64 / 1MB))
            $peakTxt = ('{0:N1}' -f ($pp.PeakWorkingSet64 / 1MB))
        } catch { $wsSettledTxt = 'ERR' }
        $needSettle = ($wsSettledTxt -eq 'PENDING')
    }
    # Stop only when everything this run came for is in: the fixed-offset sample,
    # and the first-frame mark. A blank start has no frame to wait for, so the
    # settle sample alone ends it; with -SettleWsMs 0 the old behaviour holds
    # (run until HoldMs).
    if (-not $needSettle -and ($mark.frame -ge 0 -or ($Image -eq '' -and $SettleWsMs -gt 0))) { break }
}
$ranMs = $sw.Elapsed.TotalMilliseconds

if ($hwnd -ne [IntPtr]::Zero) {
    [void][WinStartup]::PostMessage($hwnd, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero)
}
$closed = $false
for ($i = 0; $i -lt 20; $i++) {
    Start-Sleep -Milliseconds 100
    if (-not (Get-Process -Id $thePid -ErrorAction SilentlyContinue)) { $closed = $true; break }
}

$imgName = if ($Image -eq '') { 'blank' } else { Split-Path $Image -Leaf }
$line = 'STARTUP tag={0} img={1} spawn={2} win={3} log={4} open={5} plug={6}/{7}({8}) frame={9} cost={10} ws={11} wsSettled={13} peak={14} pollGap={12}' -f `
    $Tag, $imgName, ('{0:N0}' -f $spawnMs),
    ('{0:N0}' -f $mark.win), ('{0:N0}' -f $mark.log), ('{0:N0}' -f $mark.open),
    ('{0:N0}' -f $mark.plugFirst), ('{0:N0}' -f $mark.plugLast), $plugCount,
    ('{0:N0}' -f $mark.frame), $costTxt, $wsTxt, ('{0:N1}' -f $maxGap), $wsSettledTxt, $peakTxt
Write-Output $line
Write-Output ("RAN ms={0:N0} closed={1}" -f $ranMs, $closed)
if ($Ledger -ne '') { Add-Content -Path $Ledger -Value $line -Encoding utf8 }
