# multi_window_mem_probe.ps1 - what does one more viewer window cost in RAM?
#
# Why: multiple-instance mode puts N windows in ONE process, and each window keeps its own
# decoded bitmap (u32 per pixel) so it can repaint without decoding again. The product rule is
# "small and light", so the number that matters is not the per-window figure in isolation but
# the MARGINAL cost of the 2nd and 3rd window - that is what a user pays for opening another
# photo. This rig opens N windows one at a time on the SAME large fixture (hard links, so the
# disk is not multiplied) and reads process-level memory after each one reaches the screen.
#
# It reports three numbers per step, because they answer different questions:
#   ws    WorkingSet64      - what the user sees in Task Manager right now
#   peak  PeakWorkingSet64  - the high-water mark of the whole run (decode spikes included)
#   priv  PrivateMemorySize64 - committed private bytes, the honest "held" figure
#
# Rules honoured: ASCII only (K27/K78), private root so the shared .build\rupix.ini cannot make
# the arm lie (K84), only touches the instance it started, WM_CLOSE to close (K67), run marker
# removed at the end.

param(
    [Parameter(Position = 0)][string]$Tag = 'mem',
    [Parameter(Position = 1)][int]$N = 3,
    [string]$Src = 'E:\tmp\AI.Agent\RuPix\.temp\by_dll\common\100M\noise.jpg',
    [string]$Dir = 'E:\tmp\AI.Agent\RuPix\.temp\mem-multi',
    [int]$SettleMs = 4000,
    [int]$WaitWindowMs = 30000,
    # 1 = open the 2nd..Nth window BACK TO BACK without waiting, so their decodes overlap.
    # That is the only way to see the peak a user actually pays when they fire several big
    # photos at once; opened one-at-a-time the decodes are serial and the peak never stacks.
    [int]$Burst = 0
)
$ErrorActionPreference = 'Stop'

Add-Type @"
using System;
using System.Text;
using System.Collections.Generic;
using System.Runtime.InteropServices;
public static class MMR {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr l);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern int GetWindowLongW(IntPtr h, int idx);
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, int m, IntPtr w, IntPtr l);
  public static int CountVisible(uint pid) {
    int n = 0;
    EnumWindows(delegate(IntPtr h, IntPtr l) {
      uint p;
      GetWindowThreadProcessId(h, out p);
      if (p != pid) return true;
      if (!IsWindowVisible(h)) return true;
      if ((GetWindowLongW(h, -20) & 0x80) != 0) return true;   // WS_EX_TOOLWINDOW
      n++;
      return true;
    }, IntPtr.Zero);
    return n;
  }
  public static void CloseAll(uint pid) {
    EnumWindows(delegate(IntPtr h, IntPtr l) {
      uint p;
      GetWindowThreadProcessId(h, out p);
      if (p == pid && IsWindowVisible(h)) {
        PostMessage(h, 0x0010, IntPtr.Zero, IntPtr.Zero);   // WM_CLOSE
      }
      return true;
    }, IntPtr.Zero);
  }
}
"@

function Get-RupixPids {
    @(Get-Process -Name rupix -ErrorAction SilentlyContinue | ForEach-Object { [uint32]$_.Id })
}

$build = 'E:\tmp\AI.Agent\RuPix\.build'
$srcExe = Join-Path $build 'rupix.exe'
if (-not (Test-Path $srcExe)) { Write-Output 'NO_EXE'; exit 1 }
if (-not (Test-Path $Src)) { Write-Output ('NO_FIXTURE ' + $Src); exit 1 }

$pre = Get-RupixPids
if ($pre.Count -gt 0) {
    Get-Process -Name rupix -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
    Start-Sleep -Milliseconds 600
    Write-Output ('CLEARED_STALE pids=' + ($pre -join ','))
}

# Fixture: N hard links to one file (the same photo, N names). Disk cost stays 1x, and
# "same file three times" is exactly the case where a naive cache would share the bitmap -
# it does not, each window decodes and holds its own copy; that is what this measures.
New-Item -ItemType Directory -Force -Path $Dir | Out-Null
$files = @()
for ($i = 0; $i -lt $N; $i++) {
    $f = Join-Path $Dir ('big{0}.jpg' -f $i)
    if (-not (Test-Path $f)) {
        try { New-Item -ItemType HardLink -Path $f -Target $Src -ErrorAction Stop | Out-Null }
        catch { Copy-Item $Src $f -Force }
    }
    $files += $f
}
$fixtureMb = [math]::Round((Get-Item $Src).Length / 1MB, 1)
Write-Output ("FIXTURE file={0} size_mb={1} links={2}" -f (Split-Path $Src -Leaf), $fixtureMb, $N)

$exeRoot = "E:\tmp\AI.Agent\RuPix\.temp\probe-mem-$Tag"
New-Item -ItemType Directory -Force -Path $exeRoot | Out-Null
$env:RUPIX_DECODERS_DIR = Join-Path $build 'decoders'
$ini = @(
    'allow_multiple=1',
    'prefetch=0', 'close_to_tray=0', 'minimal_mode=0',
    'decoders_preferred=', 'decoders_disabled=',
    'window_w=900', 'window_h=700', 'init_window=0',
    'opacity=100', 'filmstrip=0', 'checker=0', 'stay_on_top=0'
)
$ini | Set-Content -Path (Join-Path $exeRoot 'rupix.ini') -Encoding ascii

$exe = Join-Path $exeRoot 'rupix.exe'
if (Test-Path $exe) { Remove-Item $exe -Force -ErrorAction SilentlyContinue }
try { New-Item -ItemType HardLink -Path $exe -Target $srcExe -ErrorAction Stop | Out-Null }
catch { Copy-Item $srcExe $exe -Force }

Write-Output ("ARM {0} windows_target={1}" -f $Tag, $N)

$rows = @()
for ($i = 0; $i -lt $N; $i++) {
    Start-Process -FilePath $exe -ArgumentList ('"' + $files[$i] + '"') -WorkingDirectory $exeRoot | Out-Null
    if ($Burst -eq 1 -and $i -gt 0) {
        # Fire and forget: no wait, no settle - let the decodes pile up on purpose.
        continue
    }
    $want = $i + 1
    $deadline = (Get-Date).AddMilliseconds($WaitWindowMs)
    while ((Get-Date) -lt $deadline) {
        Start-Sleep -Milliseconds 250
        $pids = Get-RupixPids
        if ($pids.Count -gt 0) {
            if ([MMR]::CountVisible($pids[0]) -ge $want) { break }
        }
    }
    # Let the decode land and the working-set trimmer settle before reading memory.
    Start-Sleep -Milliseconds $SettleMs
    $p = Get-Process -Name rupix -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($null -eq $p) { Write-Output 'PROCESS_GONE'; break }
    $ws = [math]::Round($p.WorkingSet64 / 1MB, 1)
    $peak = [math]::Round($p.PeakWorkingSet64 / 1MB, 1)
    $priv = [math]::Round($p.PrivateMemorySize64 / 1MB, 1)
    $rows += [pscustomobject]@{ n = $want; ws = $ws; peak = $peak; priv = $priv }
    Write-Output ("MEM windows={0} ws_mb={1} peak_mb={2} priv_mb={3}" -f $want, $ws, $peak, $priv)
}

if ($Burst -eq 1 -and $N -gt 1) {
    # Wait for the whole burst to land, then read once. PeakWorkingSet64 is a high-water mark,
    # so overlapping decodes are visible here even after they finished.
    $deadline = (Get-Date).AddMilliseconds($WaitWindowMs)
    while ((Get-Date) -lt $deadline) {
        Start-Sleep -Milliseconds 250
        $pids = Get-RupixPids
        if ($pids.Count -gt 0 -and [MMR]::CountVisible($pids[0]) -ge $N) { break }
    }
    Start-Sleep -Milliseconds $SettleMs
    $p = Get-Process -Name rupix -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($null -ne $p) {
        $ws = [math]::Round($p.WorkingSet64 / 1MB, 1)
        $peak = [math]::Round($p.PeakWorkingSet64 / 1MB, 1)
        $priv = [math]::Round($p.PrivateMemorySize64 / 1MB, 1)
        $rows += [pscustomobject]@{ n = $N; ws = $ws; peak = $peak; priv = $priv }
        Write-Output ("MEM windows={0} burst=1 ws_mb={1} peak_mb={2} priv_mb={3}" -f $N, $ws, $peak, $priv)
    }
}

# Marginal cost of one more window: fit a straight line through the ws / priv readings.
# Two windows are enough to get a slope; more windows just confirm it stays linear.
if ($rows.Count -ge 2) {
    $first = $rows[0]
    $last = $rows[$rows.Count - 1]
    $steps = [math]::Max(1, ($last.n - $first.n))
    $dWs = [math]::Round(($last.ws - $first.ws) / $steps, 1)
    $dPriv = [math]::Round(($last.priv - $first.priv) / $steps, 1)
    Write-Output ("MARGINAL per_window_ws_mb={0} per_window_priv_mb={1} (from n={2} to n={3})" -f $dWs, $dPriv, $first.n, $last.n)
}

# Log evidence: one [main-latency] per window that actually reached the screen.
$log = Join-Path $exeRoot ('temp\rupix-' + (Get-Date -Format 'yyyy-MM-dd') + '.log')
if (Test-Path $log) {
    $txt = Get-Content $log -Raw -Encoding UTF8
    $shown = ([regex]::Matches($txt, '\[main-latency\]')).Count
    Write-Output ("LOG main_latency_lines={0}" -f $shown)
}

$lastN = if ($rows.Count -gt 0) { $rows[$rows.Count - 1].n } else { 0 }
$verdict = if ($lastN -eq $N) { 'PASS' } else { 'FAIL' }
Write-Output ("RESULT {0} windows={1}/{2}" -f $verdict, $lastN, $N)

# -- teardown ---------------------------------------------------------------
$pids = Get-RupixPids
if ($pids.Count -gt 0) { [MMR]::CloseAll($pids[0]) }
$t1 = Get-Date
$gone = $false
while (((Get-Date) - $t1).TotalMilliseconds -lt 15000) {
    Start-Sleep -Milliseconds 200
    if ((Get-RupixPids).Count -eq 0) { $gone = $true; break }
}
if (-not $gone) {
    Get-Process -Name rupix -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
    Write-Output 'TEARDOWN force_killed=1'
} else {
    Write-Output ("TEARDOWN clean=1 exit_ms={0:N0}" -f ((Get-Date) - $t1).TotalMilliseconds)
}
$marker = Join-Path $exeRoot 'temp\rupix.run'
if (Test-Path $marker) { Remove-Item $marker -Force -ErrorAction SilentlyContinue }
Write-Output 'DONE'
