# metadata_probe.ps1 - prove that two viewer windows can EACH own a "properties" dialog.
#
# Why: the properties window used a single app-wide key ("metadata") AND a single set of text
# signals. With multiple instances on, window B's properties request just focused window A's
# dialog, and even if it had opened, both would have shown the same file. m232 scoped both the
# key and the signal set per window.
#
# How: bind Action::Properties to F2 through the private root's ini ("hotkeys=props:F2"), open
# two images in two windows, then tap F2 into each window with PostMessage. Count the windows
# whose title is NOT the main window title: expect 2 dialogs.
#
# ASCII only (K27/K78), private root (K84), WM_CLOSE teardown, run marker removed.

param(
    [Parameter(Position = 0)][string]$Tag = 'meta',
    [string]$Dir = 'E:\tmp\AI.Agent\RuPix\.temp\m229-multi',
    [int]$SettleMs = 2500,
    [int]$WaitWindowMs = 20000
)
$ErrorActionPreference = 'Stop'

Add-Type @"
using System;
using System.Text;
using System.Collections.Generic;
using System.Runtime.InteropServices;
public static class MP {
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
  // WM_KEYDOWN / WM_KEYUP for a virtual key. Function keys travel the key path (not WM_CHAR).
  public static void Tap(IntPtr h, int vk) {
    PostMessage(h, 0x0100, new IntPtr(vk), new IntPtr(0x00010001));
    PostMessage(h, 0x0101, new IntPtr(vk), new IntPtr(unchecked((long)0xC0010001)));
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
    @([MP]::Snapshot($Pids))
}

$build = 'E:\tmp\AI.Agent\RuPix\.build'
$srcExe = Join-Path $build 'rupix.exe'
if (-not (Test-Path $srcExe)) { Write-Output 'NO_EXE'; exit 1 }
$pre = Get-RupixPids
if ($pre.Count -gt 0) { Write-Output ('ABORT_A_RUPIX_IS_RUNNING pids=' + ($pre -join ',')); exit 1 }

$files = @(Get-ChildItem $Dir -Filter *.jpg -File | Sort-Object Name | Select-Object -First 2)
if ($files.Count -lt 2) { Write-Output 'NOT_ENOUGH_FILES'; exit 1 }

$exeRoot = "E:\tmp\AI.Agent\RuPix\.temp\probe-meta-$Tag"
New-Item -ItemType Directory -Force -Path $exeRoot | Out-Null
$env:RUPIX_DECODERS_DIR = Join-Path $build 'decoders'
$ini = @(
    'allow_multiple=1',
    'hotkeys=props:F2',
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

foreach ($f in $files) {
    Start-Process -FilePath $exe -ArgumentList ('"' + $f.FullName + '"') -WorkingDirectory $exeRoot | Out-Null
    $deadline = (Get-Date).AddMilliseconds($WaitWindowMs)
    while ((Get-Date) -lt $deadline) {
        Start-Sleep -Milliseconds 250
        if ((Get-Windows -Pids (Get-RupixPids)).Count -ge 2) { break }
    }
}
Start-Sleep -Milliseconds $SettleMs

$rows = Get-Windows -Pids (Get-RupixPids)
Write-Output ("MAINS {0}" -f $rows.Count)
if ($rows.Count -ne 2) { Write-Output 'ABORT_NOT_TWO_WINDOWS'; [MP]::CloseAll((Get-RupixPids)); exit 1 }

# F2 into the FIRST window, then F2 into the SECOND one.
foreach ($row in $rows) {
    $h = [IntPtr]([long](($row -split '\|')[0]))
    [MP]::Tap($h, 0x71)
    Start-Sleep -Milliseconds 1200
}

Start-Sleep -Milliseconds $SettleMs
$rows = Get-Windows -Pids (Get-RupixPids)
$dialogs = @($rows | Where-Object { (($_ -split '\|')[2]) -ne 'RuPix' })
$titles = @($rows | ForEach-Object { ($_ -split '\|')[2] })
Write-Output ("AFTER_F2 windows={0} dialogs={1} titles={2}" -f $rows.Count, $dialogs.Count, ($titles -join ';'))

$verdict = if ($dialogs.Count -eq 2) { 'PASS' } else { 'FAIL' }
Write-Output ("RESULT {0} dialogs={1}" -f $verdict, $dialogs.Count)

# -- teardown ---------------------------------------------------------------
[MP]::CloseAll((Get-RupixPids))
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
