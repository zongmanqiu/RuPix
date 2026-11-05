# dll_serial_probe.ps1 - drive one or two windows over SAME-EXTENSION images and capture
# the host log that proves whether two decodes ever sat inside the same decoder DLL.
#
# Why: m229 added a per-DLL lock because the ABI contract promises "host calls are serial,
# decoders need not be MT-safe". The product rule is "multiple windows, same extension is
# legal" - so the interesting question is not whether two .jpg windows open (they do), but
# whether the lock keeps them working while they queue. This rig produces the raw bytes;
# dll_serial_analyze.py turns them into C1..C6.
#
# Arms:
#   -Arm single : one window, RIGHT xN            -> baseline cost + zero contention
#   -Arm dual   : two windows, A RIGHT / B LEFT   -> head-on, maximum overlap
#
# Keys: VK_RIGHT(39) / VK_LEFT(37) via PostMessage to each HWND (no focus stealing).
# Character keys would NOT work: actions come from WM_CHAR only (K192).
#
# Rules honoured: ASCII only (K27/K78), private root so the shared .build\rupix.ini cannot
# make the arm lie (K84), only touches the instance it started, WM_CLOSE teardown (K67),
# run marker removed at the end.

param(
    [Parameter(Position = 0)][ValidateSet('single', 'dual', 'triple', 'idle')][string]$Arm = 'dual',
    [Parameter(Position = 1)][int]$N = 40,
    [Parameter(Position = 2)][int]$IntervalMs = 250,
    [string]$Tag = 'dual',
    [string]$Dir = 'E:\tmp\AI.Agent\RuPix\.temp\m229-multi',
    [string]$Filter = '*.jpg',
    # A's own pattern stays .jpg unless asked otherwise; only the B folder picks up $Filter.
    [string]$AFilter = '*.jpg',
    # B/C may walk a DIFFERENT folder on purpose: that is how the cross-DLL arm is built
    # (A on .jpg = common, B on .avif = modern). Empty = same folder as A.
    [string]$BDir = '',
    [string]$CDir = '',
    # S3 arm: close window B halfway through the cadence (decode in flight) and keep driving A.
    [switch]$CloseMidway,
    # 0 (default, keeps the m229/m230 arms reproducible) / 1 = the SHIPPED default.
    # This matters: with prefetch on, every idle window decodes its neighbours through the
    # SAME per-DLL lock, so a user's own turn can queue behind somebody else's prefetch.
    [int]$Prefetch = 0,
    # Binary under test. Empty = the deployed .build\rupix.exe; pass a path to A/B two builds
    # (named -Bin, not -Exe: PS variable names are case-insensitive and $exe is the private
    # copy this script launches).
    [string]$Bin = '',
    [int]$SettleSec = 6,
    [int]$HoldSec = 0,
    [int]$WindowW = 900,
    [int]$WindowH = 700
)
$ErrorActionPreference = 'Stop'

Add-Type @"
using System;
using System.Text;
using System.Collections.Generic;
using System.Runtime.InteropServices;
public static class DS {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr l);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern int GetWindowLongW(IntPtr h, int idx);
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, int m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern uint MapVirtualKeyW(uint vk, uint mapType);

  public static string[] Snapshot(uint[] pids) {
    var list = new List<string>();
    EnumWindows(delegate(IntPtr h, IntPtr l) {
      uint pid;
      GetWindowThreadProcessId(h, out pid);
      bool mine = false;
      for (int i = 0; i < pids.Length; i++) { if (pids[i] == pid) { mine = true; break; } }
      if (!mine) return true;
      if (!IsWindowVisible(h)) return true;
      if ((GetWindowLongW(h, -20) & 0x80) != 0) return true;
      var sb = new StringBuilder(512);
      GetWindowTextW(h, sb, 512);
      if (sb.Length == 0) return true;
      list.Add(h.ToInt64().ToString() + "|" + pid + "|" + sb.ToString());
      return true;
    }, IntPtr.Zero);
    return list.ToArray();
  }
  public static void CloseAll(uint[] pids) {
    foreach (string row in Snapshot(pids)) {
      PostMessage(new IntPtr(long.Parse(row.Split('|')[0])), 0x0010, IntPtr.Zero, IntPtr.Zero);
    }
  }
  // One key press: DOWN then UP, scan code from the layout (K192).
  public static void Tap(IntPtr h, int vk) {
    uint sc = MapVirtualKeyW((uint)vk, 1) & 0xffff;
    PostMessage(h, 0x0100, new IntPtr(vk), new IntPtr((long)1 | ((long)sc << 16) | 0x40000000L));
    PostMessage(h, 0x0101, new IntPtr(vk), new IntPtr((long)1 | ((long)sc << 16) | 0xC0000000L));
  }
}
"@

function Get-RupixPids {
    @(Get-Process -Name rupix -ErrorAction SilentlyContinue | ForEach-Object { [uint32]$_.Id })
}
function Get-Hwnds {
    $pids = Get-RupixPids
    if ($null -eq $pids -or $pids.Count -eq 0) { return @() }
    @([DS]::Snapshot($pids) | ForEach-Object { [IntPtr][long]($_ -split '\|')[0] })
}

$build = 'E:\tmp\AI.Agent\RuPix\.build'
$srcExe = if ($Bin -ne '') { $Bin } else { Join-Path $build 'rupix.exe' }
if (-not (Test-Path $srcExe)) { Write-Output 'NO_EXE'; exit 1 }

function Pick-Pair {
    param([string]$Root, [string]$Pat)
    $set = @(Get-ChildItem $Root -Filter $Pat -File | Sort-Object Name)
    if ($set.Count -lt 4) { return $null }
    return @{ First = $set[0].FullName; Last = $set[$set.Count - 1].FullName; Mid = $set[[int]($set.Count / 2)].FullName }
}

# A always walks $Dir with $AFilter; B may walk $BDir with $Filter (the cross-DLL arm needs
# two different extensions on purpose, so the two filters must be independent).
$a = Pick-Pair $Dir $AFilter
if ($null -eq $a) { Write-Output ('NOT_ENOUGH_FILES ' + $Dir + ' ' + $AFilter); exit 1 }
$b = if ($BDir -ne '') { Pick-Pair $BDir $Filter } else { $a }
if ($null -eq $b) { Write-Output ('NOT_ENOUGH_FILES ' + $BDir + ' ' + $Filter); exit 1 }
$c = if ($CDir -ne '') { Pick-Pair $CDir $AFilter } else { $a }
$first = $a.First
$last = $b.Last

$pre = Get-RupixPids
if ($pre.Count -gt 0) { Write-Output ('ABORT_A_RUPIX_IS_RUNNING pids=' + ($pre -join ',')); exit 1 }

$exeRoot = "E:\tmp\AI.Agent\RuPix\.temp\probe-soak-$Tag"
New-Item -ItemType Directory -Force -Path $exeRoot | Out-Null
$env:RUPIX_DECODERS_DIR = Join-Path $build 'decoders'
$env:RUST_LOG = 'info,fontdb=error'
$env:RUPIX_DECODE_TRACE = '1'
@(
    'allow_multiple=1', ("prefetch={0}" -f $Prefetch), 'close_to_tray=0', 'minimal_mode=0',
    'decoders_preferred=', 'decoders_disabled=',
    ("window_w={0}" -f $WindowW), ("window_h={0}" -f $WindowH), 'init_window=0',
    'opacity=100', 'filmstrip=0', 'checker=0', 'stay_on_top=0'
) | Set-Content -Path (Join-Path $exeRoot 'rupix.ini') -Encoding ascii

$exe = Join-Path $exeRoot 'rupix.exe'
if (-not (Test-Path $exe)) {
    try { New-Item -ItemType HardLink -Path $exe -Target $srcExe -ErrorAction Stop | Out-Null }
    catch { Copy-Item $srcExe $exe -Force }
}

$log = Join-Path $exeRoot ('temp\rupix-' + (Get-Date -Format 'yyyy-MM-dd') + '.log')
if (Test-Path $log) { Remove-Item $log -Force }   # this arm owns the private root; start clean

Write-Output ("ARM {0} n={1} interval={2}ms dir={3}" -f $Arm, $N, $IntervalMs, $Dir)

$proc = Start-Process -FilePath $exe -ArgumentList ('"' + $first + '"') -PassThru -WorkingDirectory $exeRoot
$deadline = (Get-Date).AddSeconds(30)
while ((Get-Hwnds).Count -lt 1 -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 200 }
$hA = (Get-Hwnds)[0]
if ($null -eq $hA) { Write-Output 'ABORT_NO_WINDOW'; exit 1 }
Write-Output ("WINDOW A hwnd={0}" -f $hA)

$hB = [IntPtr]::Zero
$hC = [IntPtr]::Zero
if ($Arm -ne 'single') {
    $before = Get-Hwnds
    Start-Process -FilePath $exe -ArgumentList ('"' + $last + '"') -WorkingDirectory $exeRoot | Out-Null
    $deadline = (Get-Date).AddSeconds(30)
    while ((Get-Date) -lt $deadline) {
        $now = Get-Hwnds
        if ($now.Count -ge 2) {
            $hB = @($now | Where-Object { $before -notcontains $_ })[0]
            break
        }
        Start-Sleep -Milliseconds 200
    }
    if ($hB -eq [IntPtr]::Zero) { Write-Output 'ABORT_NO_SECOND_WINDOW'; exit 1 }
    Write-Output ("WINDOW B hwnd={0} file={1}" -f $hB, [System.IO.Path]::GetFileName($last))
}

if ($Arm -eq 'triple') {
    $before = Get-Hwnds
    $mid = $c.Mid
    Start-Process -FilePath $exe -ArgumentList ('"' + $mid + '"') -WorkingDirectory $exeRoot | Out-Null
    $deadline = (Get-Date).AddSeconds(30)
    while ((Get-Date) -lt $deadline) {
        $now = Get-Hwnds
        if ($now.Count -ge 3) {
            $hC = @($now | Where-Object { $before -notcontains $_ })[0]
            break
        }
        Start-Sleep -Milliseconds 200
    }
    if ($hC -eq [IntPtr]::Zero) { Write-Output 'ABORT_NO_THIRD_WINDOW'; exit 1 }
    Write-Output ("WINDOW C hwnd={0} file={1}" -f $hC, [System.IO.Path]::GetFileName($mid))
}

# Let the first images land before the cadence: decoding a 26 MP jpg takes ~1.2 s on this box.
Start-Sleep -Seconds $SettleSec

$right = 39
$left = 37
$taps = 0
$t0 = Get-Date
if ($Arm -eq 'single' -or $Arm -eq 'idle') {
    # 'idle': a second window is open but NEVER driven - it sits there prefetching its
    # neighbours. That is the case the user actually hits ("one window flips, another is
    # just open"), and the one where a background prefetch can stand in the user's way.
    for ($i = 0; $i -lt $N; $i++) {
        [DS]::Tap($hA, $right); $taps++
        Start-Sleep -Milliseconds $IntervalMs
    }
} elseif ($Arm -eq 'dual') {
    # Head-on: A walks forward from the first file, B walks backward from the last, so the
    # two windows keep hitting the folder's middle at the same time.
    $closedAt = -1
    for ($i = 0; $i -lt $N; $i++) {
        if ($CloseMidway -and $closedAt -lt 0 -and $i -ge [int]($N / 2)) {
            # S3: tear B down while its last decode may still be inside the DLL.
            [void][DS]::PostMessage($hB, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero)
            $closedAt = $i
            Write-Output ("CLOSED_B at_tap={0}" -f $i)
        }
        [DS]::Tap($hA, $right); $taps++
        Start-Sleep -Milliseconds ([int]($IntervalMs / 2))
        if ($closedAt -lt 0) { [DS]::Tap($hB, $left); $taps++ }
        Start-Sleep -Milliseconds ([int]($IntervalMs / 2))
    }
} else {
    # Three windows, three starting points, all on the same decoder DLL (worst case for the queue).
    for ($i = 0; $i -lt $N; $i++) {
        [DS]::Tap($hA, $right); $taps++
        Start-Sleep -Milliseconds ([int]($IntervalMs / 3))
        [DS]::Tap($hB, $left); $taps++
        Start-Sleep -Milliseconds ([int]($IntervalMs / 3))
        [DS]::Tap($hC, $right); $taps++
        Start-Sleep -Milliseconds ([int]($IntervalMs / 3))
    }
}
$span = ((Get-Date) - $t0).TotalSeconds
Write-Output ("DRIVEN taps={0} span={1:N1}s end={2}" -f $taps, $span, (Get-Date -Format 'HH:mm:ss.fff'))

# Wait for the queue to drain: log size unchanged for 3 samples of 2 s, cap 180 s.
$lastLen = -1L
$still = 0
for ($i = 0; $i -lt 90; $i++) {
    Start-Sleep -Seconds 2
    $cur = (Get-Item $log).Length
    if ($cur -eq $lastLen) { $still++ } else { $still = 0; $lastLen = $cur }
    if ($still -ge 3) { break }
}

if ($HoldSec -gt 0) {
    # Keep the process alive after the drive. The watchdog's FIRST report is at 10 s, so a
    # run that tears down right after the drive can never prove "no spurious report" - it
    # only proves we did not wait long enough. -HoldSec 14 makes that claim testable.
    Write-Output ("HOLD {0}s" -f $HoldSec)
    Start-Sleep -Seconds $HoldSec
}

$out = "E:\tmp\AI.Agent\RuPix\.temp\soak-m229\$Tag.log"
New-Item -ItemType Directory -Force -Path (Split-Path $out) | Out-Null
Copy-Item $log $out -Force
Write-Output ("CAPTURED {0} bytes={1}" -f $out, (Get-Item $out).Length)

# What actually reached the screen: one [main-latency] per shown image, with its timestamp.
# For the -CloseMidway arm the interesting question is whether A kept producing lines AFTER
# the B window went away (a leaked lock would show up as "occupied" errors instead).
$txt = Get-Content $out -Raw -Encoding UTF8
$lat = @([regex]::Matches($txt, '\[(\d\d:\d\d:\d\d\.\d\d\d)\].*?\[main-latency\] file=(\S+)') |
    ForEach-Object { ($_.Groups[1].Value + ' ' + [System.IO.Path]::GetFileName($_.Groups[2].Value)) })
$lockErr = ([regex]::Matches($txt, '\[dll-lock\]')).Count
Write-Output ("LOG latency_lines={0} lock_errors={1}" -f $lat.Count, $lockErr)
if ($lat.Count -gt 0) {
    Write-Output ("LOG last_latency=" + $lat[$lat.Count - 1])
}

$pids = Get-RupixPids
[DS]::CloseAll($pids)
$t0 = Get-Date
$gone = $false
while (((Get-Date) - $t0).TotalMilliseconds -lt 20000) {
    Start-Sleep -Milliseconds 200
    if ((Get-RupixPids).Count -eq 0) { $gone = $true; break }
}
if (-not $gone) {
    Get-Process -Name rupix -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
    Write-Output 'TEARDOWN force_killed=1'
} else {
    Write-Output ("TEARDOWN clean=1 exit_ms={0:N0}" -f ((Get-Date) - $t0).TotalMilliseconds)
}
$marker = Join-Path $exeRoot 'temp\rupix.run'
if (Test-Path $marker) { Remove-Item $marker -Force -ErrorAction SilentlyContinue }
Write-Output 'DONE'
