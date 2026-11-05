# dir_rescan_probe.ps1 - how much does ONE file change in a big folder cost when N windows
# are watching that same folder?
#
# Why: every viewer window owns its own DirWatcher and its own 50 ms rescan throttle, so with
# N windows on one folder a single file change is reported N times and rescanned N times -
# all on the one UI thread. That is the only multi-window cost that is both unique to the
# feature and unmeasured. This rig measures it instead of asserting it.
#
# Method: build a folder of -Count entries (hard links to one real .jpg, so it costs no disk),
# open -Windows windows on that same folder with multiple-instance on, then make ONE change
# (add one more .jpg) and read the [dir-rescan] lines the host emits (RUPIX_DIR_TRACE=1).
#
#   lines       -> how many windows rescanned (N means the work is duplicated N times)
#   sum_scan    -> total enumeration+sort time paid on the UI thread
#   max_scan    -> one scan's cost; sum_scan/max_scan is the duplication factor
#
# Two ticks are driven, so "the first change after warm-up" and "a later change" can be told
# apart (the first one may include cold metadata reads and is reported separately).
#
# Rules honoured: ASCII only (K27/K78), private root so the shared .build\rupix.ini cannot
# make the arm lie (K84), only touches the instance it started, WM_CLOSE to close (K67),
# and the run marker is removed at the end.

param(
    [Parameter(Position = 0)][int]$Windows = 3,
    [Parameter(Position = 1)][string]$Tag = 'dir1',
    [string]$Dir = 'E:\tmp\AI.Agent\RuPix\.temp\m234-dir',
    [int]$Count = 3000,
    [int]$SettleMs = 3000,
    [int]$AfterChangeMs = 1600
)
$ErrorActionPreference = 'Stop'

Add-Type @"
using System;
using System.Text;
using System.Collections.Generic;
using System.Runtime.InteropServices;
public static class DR {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr l);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern int GetWindowLongW(IntPtr h, int idx);
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, int m, IntPtr w, IntPtr l);

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
      string[] parts = row.Split('|');
      PostMessage(new IntPtr(long.Parse(parts[0])), 0x0010, IntPtr.Zero, IntPtr.Zero);
    }
  }
}
"@

function Get-RupixPids {
    @(Get-Process -Name rupix -ErrorAction SilentlyContinue | ForEach-Object { [uint32]$_.Id })
}

$build = 'E:\tmp\AI.Agent\RuPix\.build'
$srcExe = Join-Path $build 'rupix.exe'
if (-not (Test-Path $srcExe)) { Write-Output 'NO_EXE'; exit 1 }

$seed = Get-ChildItem $build -Filter *.jpg -File -Recurse -ErrorAction SilentlyContinue |
    Sort-Object Length | Select-Object -First 1
if ($null -eq $seed) { $seed = Get-ChildItem 'E:\tmp\AI.Agent\RuPix\.temp\m229-multi' -Filter *.jpg -File |
        Sort-Object Length | Select-Object -First 1 }
if ($null -eq $seed) { Write-Output 'NO_SEED_JPEG'; exit 1 }

# -- fixture: -Count hard links spread over several seeds ---------------------
# NTFS caps hard links per file at 1023 (measured: the naive one-seed loop dies at ~1024 with
# "too many links on the file"). Round-robin over a few real copies keeps the fixture cheap
# (a handful of real files, the rest links) without hitting that ceiling.
$LinksPerSeed = 400
$seedDir = 'E:\tmp\AI.Agent\RuPix\.temp\m234-seed'
New-Item -ItemType Directory -Force -Path $seedDir | Out-Null
$nSeeds = [int][Math]::Ceiling($Count / $LinksPerSeed)
$seeds = @()
for ($k = 0; $k -lt $nSeeds; $k++) {
    $sp = Join-Path $seedDir ("seed{0}.jpg" -f $k)
    if (-not (Test-Path $sp)) { Copy-Item $seed.FullName $sp -Force }
    $seeds += (Get-Item $sp)
}

New-Item -ItemType Directory -Force -Path $Dir | Out-Null
$existing = @(Get-ChildItem $Dir -Filter *.jpg -File).Count
if ($existing -lt $Count) {
    $swFix = [Diagnostics.Stopwatch]::StartNew()
    for ($i = $existing; $i -lt $Count; $i++) {
        $p = Join-Path $Dir ('f{0:d5}.jpg' -f $i)
        if (-not (Test-Path $p)) {
            New-Item -ItemType HardLink -Path $p -Target $seeds[$i % $nSeeds].FullName | Out-Null
        }
    }
    $swFix.Stop()
    Write-Output ("FIXTURE built={0} ms={1:N0} seeds={2} seed={3}" -f $Count, $swFix.ElapsedMilliseconds, $nSeeds, $seed.Name)
} else {
    Write-Output ("FIXTURE reused={0}" -f $existing)
}

$pre = Get-RupixPids
if ($pre.Count -gt 0) { Write-Output ('ABORT_A_RUPIX_IS_RUNNING pids=' + ($pre -join ',')); exit 1 }

$exeRoot = "E:\tmp\AI.Agent\RuPix\.temp\probe-dir-$Tag"
New-Item -ItemType Directory -Force -Path $exeRoot | Out-Null
$env:RUPIX_DECODERS_DIR = Join-Path $build 'decoders'
$env:RUPIX_DIR_TRACE = '1'
$ini = @(
    'allow_multiple=1',
    'prefetch=0', 'close_to_tray=0', 'minimal_mode=0',
    'decoders_preferred=', 'decoders_disabled=',
    'window_w=900', 'window_h=700', 'init_window=0',
    'opacity=100', 'filmstrip=0', 'checker=0', 'stay_on_top=0'
)
$ini | Set-Content -Path (Join-Path $exeRoot 'rupix.ini') -Encoding ascii

$exe = Join-Path $exeRoot 'rupix.exe'
if (-not (Test-Path $exe)) {
    try { New-Item -ItemType HardLink -Path $exe -Target $srcExe -ErrorAction Stop | Out-Null }
    catch { Copy-Item $srcExe $exe -Force }
}

Write-Output ("ARM windows={0} count={1} dir={2}" -f $Windows, $Count, $Dir)

# -- open N windows on the SAME folder ----------------------------------------
$opened = @(Get-ChildItem $Dir -Filter *.jpg -File | Sort-Object Name | Select-Object -First $Windows)
for ($i = 0; $i -lt $opened.Count; $i++) {
    Start-Process -FilePath $exe -ArgumentList ('"' + $opened[$i].FullName + '"') -WorkingDirectory $exeRoot | Out-Null
    $deadline = (Get-Date).AddMilliseconds(20000)
    while ((Get-Date) -lt $deadline) {
        Start-Sleep -Milliseconds 250
        $pids = Get-RupixPids
        $rows = if ($pids.Count -eq 0) { @() } else { @([DR]::Snapshot($pids)) }
        if ($rows.Count -ge ($i + 1)) { break }
    }
}
Start-Sleep -Milliseconds $SettleMs
$pids = Get-RupixPids
$rows = if ($pids.Count -eq 0) { @() } else { @([DR]::Snapshot($pids)) }
Write-Output ("WINDOWS open={0} expected={1}" -f $rows.Count, $Windows)

$log = Join-Path $exeRoot ('temp\rupix-' + (Get-Date -Format 'yyyy-MM-dd') + '.log')
if (-not (Test-Path $log)) { Write-Output 'NO_LOG'; exit 1 }

# -- one change, then measure -------------------------------------------------
# The count comes back through $script:tickEnd, NOT through the pipeline: a PS function's
# Write-Output lines are captured by the caller too, which turns "12" into a mixed array.
$script:tickEnd = 0
function Measure-Tick {
    param([string]$Label, [int]$Before)
    Start-Sleep -Milliseconds $AfterChangeMs
    $txt = Get-Content $log -Raw -Encoding UTF8
    $ms = @([regex]::Matches($txt, '\[dir-rescan\] dir=\S+ files=(\d+) total=([\d.]+)ms changed=(\d) shared=(\d)'))
    $mine = @($ms | Select-Object -Skip $Before)
    $tot = @($mine | ForEach-Object { [double]$_.Groups[2].Value })
    # shared=0 means THIS window really read the disk; shared=1 means it took another window's
    # scan. The number that matters is "how many times did the UI thread touch the disk".
    $scanned = @($mine | Where-Object { $_.Groups[4].Value -eq '0' }).Count
    $sum = if ($tot.Count -gt 0) { ($tot | Measure-Object -Sum).Sum } else { 0 }
    $max = if ($tot.Count -gt 0) { ($tot | Measure-Object -Maximum).Maximum } else { 0 }
    Write-Output ("TICK {0} lines={1} scanned={2} reused={3} sum_total={4:N2}ms max_total={5:N2}ms files={6}" -f
        $Label, $mine.Count, $scanned, ($mine.Count - $scanned), $sum, $max,
        (($mine | ForEach-Object { $_.Groups[1].Value }) -join ','))
    $script:tickEnd = $ms.Count
}

$base = ([regex]::Matches((Get-Content $log -Raw -Encoding UTF8), '\[dir-rescan\]')).Count
Write-Output ("BASE rescan_lines={0}" -f $base)

# tick 1: add one .jpg (what a user does by saving a file into the folder)
$new1 = Join-Path $Dir 'zz_tick1.jpg'
if (-not (Test-Path $new1)) { New-Item -ItemType HardLink -Path $new1 -Target $seeds[0].FullName | Out-Null }
Measure-Tick -Label 'add' -Before $base
$after1 = $script:tickEnd

# tick 2: delete it again (an independent second change - tells "first tick is special" apart)
Remove-Item -LiteralPath $new1 -Force -ErrorAction SilentlyContinue
Measure-Tick -Label 'del' -Before $after1
$after2 = $script:tickEnd

$verdict = if ($Windows -le 1) { 'BASELINE' } elseif ($after1 -ge $Windows) { 'CONFIRMED_N_TIMES' } else { 'LESS_THAN_N' }
Write-Output ("RESULT {0} windows={1} tick1_lines={2} tick2_lines={3}" -f $verdict, $Windows, $after1, ($after2 - $after1))

# -- teardown ---------------------------------------------------------------
$pids = Get-RupixPids
[DR]::CloseAll($pids)
$t0 = Get-Date
$gone = $false
while (((Get-Date) - $t0).TotalMilliseconds -lt 15000) {
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
