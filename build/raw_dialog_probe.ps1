# raw_dialog_probe.ps1 - prove that two viewer windows can EACH own a "raw pixel parameters"
# dialog, i.e. that the dialog is scoped per window instead of being one global singleton.
#
# Why: the RAW-parameter dialog used a single app-wide key ("raw-params"). With multiple
# instances on, window B's "RAW parameter..." did nothing at all while window A had its
# dialog open (windui just focuses the already-registered singleton) - a silent no-op.
# m232 scoped the key per window, so opening a headerless file in a second window must pop a
# SECOND dialog instead of focusing the first one.
#
# How: two headerless .bgra fixtures whose byte length admits no integer geometry, so both
# launches have to ASK (a file that can be inferred opens straight away - no dialog, no test).
# Then count top-level windows of the survivor process: main + dialog after launch 1,
# 2 mains + 2 dialogs after launch 2.
#
# Rules honoured: ASCII only (K27/K78), private root so the shared ini cannot make the arm
# lie (K84), only touches the instance it started, WM_CLOSE to close, run marker removed.

param(
    [Parameter(Position = 0)][string]$Tag = 'rawdlg',
    [int]$SettleMs = 2500,
    [int]$WaitWindowMs = 20000
)
$ErrorActionPreference = 'Stop'

Add-Type @"
using System;
using System.Text;
using System.Collections.Generic;
using System.Runtime.InteropServices;
public static class RD {
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
      string t = sb.ToString();
      if (t.Length == 0) return true;
      list.Add(h.ToInt64().ToString() + "|" + pid + "|" + t);
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

function Get-Windows {
    param([uint32[]]$Pids)
    if ($null -eq $Pids -or $Pids.Count -eq 0) { return @() }
    @([RD]::Snapshot($Pids))
}

$build = 'E:\tmp\AI.Agent\RuPix\.build'
$srcExe = Join-Path $build 'rupix.exe'
if (-not (Test-Path $srcExe)) { Write-Output 'NO_EXE'; exit 1 }

$pre = Get-RupixPids
if ($pre.Count -gt 0) { Write-Output ('ABORT_A_RUPIX_IS_RUNNING pids=' + ($pre -join ',')); exit 1 }

$exeRoot = "E:\tmp\AI.Agent\RuPix\.temp\probe-raw-$Tag"
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
if (-not (Test-Path $exe)) {
    try { New-Item -ItemType HardLink -Path $exe -Target $srcExe -ErrorAction Stop | Out-Null }
    catch { Copy-Item $srcExe $exe -Force }
}

# Two headerless fixtures that CANNOT be inferred: 100003 bytes is divisible by neither 4
# (bgra) nor 3 (rgb), so the host has to ask for width/height - which is the dialog.
$rawDir = Join-Path $exeRoot 'raw'
New-Item -ItemType Directory -Force -Path $rawDir | Out-Null
$buf = New-Object byte[] 100003
foreach ($n in @('r1.bgra', 'r2.bgra')) {
    $p = Join-Path $rawDir $n
    if (-not (Test-Path $p)) { [System.IO.File]::WriteAllBytes($p, $buf) }
}

Write-Output ("ARM raw_dialog launches=2 dir={0}" -f $rawDir)

$fail = 0
$wanted = 0
foreach ($n in @('r1.bgra', 'r2.bgra')) {
    $wanted++
    $p = Join-Path $rawDir $n
    Start-Process -FilePath $exe -ArgumentList ('"' + $p + '"') -WorkingDirectory $exeRoot | Out-Null
    # main window + one dialog per launch that asked for parameters
    $want = $wanted * 2
    $rows = @()
    $deadline = (Get-Date).AddMilliseconds($WaitWindowMs)
    while ((Get-Date) -lt $deadline) {
        Start-Sleep -Milliseconds 250
        $rows = Get-Windows -Pids (Get-RupixPids)
        if ($rows.Count -ge $want) { break }
    }
    Start-Sleep -Milliseconds $SettleMs
    $rows = Get-Windows -Pids (Get-RupixPids)
    $titles = @($rows | ForEach-Object { ($_ -split '\|')[2] })
    $ok = $rows.Count -eq $want
    if (-not $ok) { $fail++ }
    Write-Output ("LAUNCH {0} file={1} windows={2} want={3} match={4} titles={5}" -f $wanted, $n, $rows.Count, $want, $ok, ($titles -join ';'))
}

# The two dialogs must be DIFFERENT windows, not one window counted twice.
$rows = Get-Windows -Pids (Get-RupixPids)
$hwnds = @($rows | ForEach-Object { ($_ -split '\|')[0] } | Sort-Object -Unique)
Write-Output ("DISTINCT hwnds={0} of rows={1}" -f $hwnds.Count, $rows.Count)

$verdict = if ($fail -eq 0 -and $hwnds.Count -eq 4) { 'PASS' } else { 'FAIL' }
Write-Output ("RESULT {0} windows={1}" -f $verdict, $rows.Count)

# -- teardown ---------------------------------------------------------------
[RD]::CloseAll((Get-RupixPids))
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
