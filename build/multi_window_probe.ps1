# multi_window_probe.ps1 - prove that "allow_multiple" really opens one window per launch,
# and that those windows may all be the SAME extension.
#
# Why: the product rule is "with multiple-instance on, every launch gets its own window, and two windows
# showing .jpg files are perfectly legal". The window key is viewer-<id>, so nothing in the
# source is supposed to dedupe by extension - but "supposed to" is not evidence. This rig
# launches the shipped exe N times from a private root and counts the top-level windows that
# belong to the survivor process, then reads their titles (which carry the file names).
#
# Arms:
#   -AllowMultiple 1  -> expect N windows, titles = the N files opened, in order
#   -AllowMultiple 0  -> expect 1 window,  title  = the LAST file (the old in-place swap)
#
# Rules honoured: ASCII only (K27/K78), private root so the shared .build\rupix.ini cannot
# make the arm lie (K84), only touches the instance it started, WM_CLOSE to close so the
# session stamp is not left behind (K67), and the run marker is removed in the end.

param(
    [Parameter(Position = 0)][string]$Tag = 'multi',
    [Parameter(Position = 1)][int]$N = 3,
    [Parameter(Position = 2)][string]$Dir = 'E:\tmp\AI.Agent\RuPix\.temp\m229-multi',
    [int]$AllowMultiple = 1,
    [int]$SettleMs = 2500,
    [int]$WaitWindowMs = 20000
)
$ErrorActionPreference = 'Stop'

Add-Type @"
using System;
using System.Text;
using System.Collections.Generic;
using System.Runtime.InteropServices;
public static class MW {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr l);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern int GetWindowLongW(IntPtr h, int idx);
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, int m, IntPtr w, IntPtr l);

  // Returns "hwnd|pid|title" for every visible, non-tool top-level window of the given pids.
  public static string[] Snapshot(uint[] pids) {
    var list = new List<string>();
    EnumWindows(delegate(IntPtr h, IntPtr l) {
      uint pid;
      GetWindowThreadProcessId(h, out pid);
      bool mine = false;
      for (int i = 0; i < pids.Length; i++) { if (pids[i] == pid) { mine = true; break; } }
      if (!mine) return true;
      if (!IsWindowVisible(h)) return true;
      if ((GetWindowLongW(h, -20) & 0x80) != 0) return true;   // GWL_EXSTYLE & WS_EX_TOOLWINDOW
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
    @([MW]::Snapshot($Pids))
}

$build = 'E:\tmp\AI.Agent\RuPix\.build'
$srcExe = Join-Path $build 'rupix.exe'
if (-not (Test-Path $srcExe)) { Write-Output 'NO_EXE'; exit 1 }

$files = @(Get-ChildItem $Dir -Filter *.jpg -File | Sort-Object Name | Select-Object -First $N)
if ($files.Count -lt $N) { Write-Output ("NOT_ENOUGH_FILES have={0} need={1}" -f $files.Count, $N); exit 1 }

$pre = Get-RupixPids
if ($pre.Count -gt 0) { Write-Output ('ABORT_A_RUPIX_IS_RUNNING pids=' + ($pre -join ',')); exit 1 }

$exeRoot = "E:\tmp\AI.Agent\RuPix\.temp\probe-mw-$Tag"
New-Item -ItemType Directory -Force -Path $exeRoot | Out-Null
$env:RUPIX_DECODERS_DIR = Join-Path $build 'decoders'
$ini = @(
    ("allow_multiple={0}" -f $AllowMultiple),
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

Write-Output ("ARM {0} allow_multiple={1} launches={2} dir={3}" -f $Tag, $AllowMultiple, $N, $Dir)

$fail = 0
for ($i = 0; $i -lt $N; $i++) {
    $f = $files[$i]
    $proc = Start-Process -FilePath $exe -ArgumentList ('"' + $f.FullName + '"') -PassThru -WorkingDirectory $exeRoot
    # The 2nd..Nth processes forward argv to the survivor and exit; poll until the window
    # count stops short of rising, then settle for the image to land.
    $want = if ($AllowMultiple -eq 1) { $i + 1 } else { 1 }
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
    # The window TITLE is the fixed product name ("RuPix"), not the file name - so the
    # verdict is the window COUNT; which file each window shows is settled from the log
    # below ([main-latency] file=..., one line per window that reached the screen).
    $ok = if ($AllowMultiple -eq 1) { $rows.Count -eq ($i + 1) } else { $rows.Count -eq 1 }
    if (-not $ok) { $fail++ }
    Write-Output ("LAUNCH {0} file={1} windows={2} match={3} titles={4}" -f ($i + 1), $f.Name, $rows.Count, $ok, ($titles -join ';'))
}

# Which files actually reached the screen, and how many windows the host announced.
$log = Join-Path $exeRoot ('temp\rupix-' + (Get-Date -Format 'yyyy-MM-dd') + '.log')
if (Test-Path $log) {
    $txt = Get-Content $log -Raw -Encoding UTF8
    $shown = @([regex]::Matches($txt, '\[main-latency\] file=([^\s]+)') | ForEach-Object { [System.IO.Path]::GetFileName($_.Groups[1].Value) })
    # "\u65b0\u5f00..." = the host's "opened a new viewer window" line; written as escapes so
    # this file stays pure ASCII (K27/K78) while still matching the UTF-8 log text.
    $newWin = ([regex]::Matches($txt, '\u65b0\u5f00\u770b\u56fe\u7a97\u53e3 id=\d+')).Count
    Write-Output ("LOG displayed={0}" -f (($shown | Select-Object -Unique) -join ','))
    Write-Output ("LOG new_window_lines={0}" -f $newWin)
}

$rows = Get-Windows -Pids (Get-RupixPids)
$verdict = if ($fail -eq 0) { 'PASS' } else { 'FAIL' }
Write-Output ("RESULT {0} windows={1}" -f $verdict, $rows.Count)

# -- teardown ---------------------------------------------------------------
$pids = Get-RupixPids
[MW]::CloseAll($pids)
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
