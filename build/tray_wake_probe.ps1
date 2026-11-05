# tray_wake_probe.ps1 - prove that "bring my windows back" restores EVERY viewer window.
#
# Why: with multiple-instance on there can be N viewer windows, each keyed viewer-<id>.
# Before this change the wake path handed the platform exactly ONE request (shared.newest()),
# so clicking the tray entry / launching rupix.exe again brought back one window and left the
# other N-1 minimized or buried. "Supposed to" is not evidence: this rig opens N windows,
# minimizes all of them, triggers the wake path, and counts how many actually came back.
#
# The wake path is exercised through the second-launch handoff (rupix.exe with no arguments),
# which is the same `wake_requests(&shared, false, None)` the tray "show windows" entry calls -
# no mouse automation of the tray menu needed.
#
# Arms:
#   -Exe <new exe>   -> expect restored = N
#   -Exe <old exe>   -> expect restored = 1   (the pre-change behaviour, for contrast)
#
# Rules honoured: ASCII only (K27/K78), private root so the shared .build\rupix.ini cannot
# make the arm lie (K84), only touches the instance it started, WM_CLOSE to close (K67),
# run marker removed at the end.

param(
    [Parameter(Position = 0)][string]$Tag = 'new',
    [Parameter(Position = 1)][int]$N = 3,
    [Parameter(Position = 2)][string]$Dir = 'E:\tmp\AI.Agent\RuPix\.temp\m229-multi',
    # NOTE: named -Bin, not -Exe: PowerShell variable names are case-insensitive, so an -Exe
    # parameter would be clobbered by the local $exe (the hard link we launch) below.
    [string]$Bin = 'E:\tmp\AI.Agent\RuPix\.build\rupix.exe',
    [int]$SettleMs = 2500,
    [int]$WaitWindowMs = 20000,
    [int]$WaitWakeMs = 8000,
    # 1 = put an unrelated window on top and read the z-order (the real judgement);
    # 0 = minimize the viewers and read IsIconic (kept because it shows the OS also
    # restores a whole group once the process regains foreground).
    [int]$Cover = 1,
    [int]$SkipWake = 0
)
$ErrorActionPreference = 'Stop'

Add-Type @"
using System;
using System.Text;
using System.Collections.Generic;
using System.Runtime.InteropServices;
public static class TW {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr l);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern int GetWindowLongW(IntPtr h, int idx);
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, int m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int cx, int cy, uint flags);

  // Put a window on top and then UNPIN it: it stays highest in the z-order but is no longer
  // a topmost window, so a later SetForegroundWindow on another window can legitimately win.
  // Activate() alone is not enough - a script-started form does not get to steal foreground.
  public static void RaiseAndUnpin(IntPtr h) {
    SetWindowPos(h, new IntPtr(-1), 0, 0, 0, 0, 0x0001 | 0x0002);   // HWND_TOPMOST, NOSIZE|NOMOVE
    SetWindowPos(h, new IntPtr(-2), 0, 0, 0, 0, 0x0001 | 0x0002);   // HWND_NOTOPMOST
  }

  // "hwnd|pid|title" for every visible, non-tool top-level window of the given pids,
  // in EnumWindows order (top of the z-order first).
  public static string[] Snapshot(uint[] pids) {
    var list = new List<string>();
    EnumWindows(delegate(IntPtr h, IntPtr l) {
      uint pid;
      GetWindowThreadProcessId(h, out pid);
      bool mine = false;
      for (int i = 0; i < pids.Length; i++) { if (pids[i] == pid) { mine = true; break; } }
      if (!mine) return true;
      if (!IsWindowVisible(h)) return true;
      if ((GetWindowLongW(h, -20) & 0x80) != 0) return true;   // WS_EX_TOOLWINDOW
      var sb = new StringBuilder(512);
      GetWindowTextW(h, sb, 512);
      if (sb.Length == 0) return true;
      list.Add(h.ToInt64().ToString() + "|" + pid + "|" + sb.ToString());
      return true;
    }, IntPtr.Zero);
    return list.ToArray();
  }
  public static int IconicCount(uint[] pids) {
    int n = 0;
    foreach (string row in Snapshot(pids)) {
      if (IsIconic(new IntPtr(long.Parse(row.Split('|')[0])))) n++;
    }
    return n;
  }
  // "hwnd:0/1;hwnd:0/1" in z-order, so a surprising count can be read instead of guessed.
  public static string IconicMap(uint[] pids) {
    var sb = new StringBuilder();
    foreach (string row in Snapshot(pids)) {
      IntPtr h = new IntPtr(long.Parse(row.Split('|')[0]));
      if (sb.Length > 0) sb.Append(';');
      sb.Append(h.ToInt64()).Append(':').Append(IsIconic(h) ? 1 : 0);
    }
    return sb.ToString();
  }
  public static void MinimizeAll(uint[] pids) {
    foreach (string row in Snapshot(pids)) {
      ShowWindow(new IntPtr(long.Parse(row.Split('|')[0])), 6);   // SW_MINIMIZE
    }
  }
  // True when the window that EnumWindows reports first is the given hwnd.
  public static bool IsTopmost(uint[] pids, long want) {
    string[] rows = Snapshot(pids);
    if (rows.Length == 0) return false;
    return long.Parse(rows[0].Split('|')[0]) == want;
  }
  // "hwnd:z;...;cover:z" - z-order index (0 = topmost among ALL top-level windows) for each
  // viewer window plus the cover window, computed in ONE EnumWindows pass so the numbers are
  // comparable. Lower index = higher on screen.
  public static string ZMap(uint[] pids, IntPtr cover) {
    var rows = new List<string>();
    int i = 0;
    EnumWindows(delegate(IntPtr h, IntPtr l) {
      uint pid;
      GetWindowThreadProcessId(h, out pid);
      bool mine = false;
      for (int k = 0; k < pids.Length; k++) { if (pids[k] == pid) { mine = true; break; } }
      if (cover != IntPtr.Zero && h == cover) {
        rows.Add("cover:" + i);
      } else if (mine && IsWindowVisible(h) && (GetWindowLongW(h, -20) & 0x80) == 0) {
        var sb = new StringBuilder(256);
        GetWindowTextW(h, sb, 256);
        if (sb.Length > 0) rows.Add(h.ToInt64().ToString() + ":" + i);
      }
      i++;
      return true;
    }, IntPtr.Zero);
    return string.Join(";", rows.ToArray());
  }
  public static void CloseAll(uint[] pids) {
    foreach (string row in Snapshot(pids)) {
      PostMessage(new IntPtr(long.Parse(row.Split('|')[0])), 0x0010, IntPtr.Zero, IntPtr.Zero);
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
    @([TW]::Snapshot($Pids))
}

$build = 'E:\tmp\AI.Agent\RuPix\.build'
if (-not (Test-Path $Bin)) { Write-Output ('NO_BIN ' + $Bin); exit 1 }

$files = @(Get-ChildItem $Dir -Filter *.jpg -File | Sort-Object Name | Select-Object -First $N)
if ($files.Count -lt $N) { Write-Output ("NOT_ENOUGH_FILES have={0} need={1}" -f $files.Count, $N); exit 1 }

$pre = Get-RupixPids
if ($pre.Count -gt 0) {
    Get-Process -Name rupix -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
    Start-Sleep -Milliseconds 600
    Write-Output ('CLEARED_STALE pids=' + ($pre -join ','))
}

$exeRoot = "E:\tmp\AI.Agent\RuPix\.temp\probe-wake-$Tag"
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

# Each arm gets its own hard link so the exe path (and therefore the process name) is rupix.exe
# while the binary under test stays identifiable in the report.
$exe = Join-Path $exeRoot 'rupix.exe'
if (Test-Path $exe) { Remove-Item $exe -Force -ErrorAction SilentlyContinue }
try { New-Item -ItemType HardLink -Path $exe -Target $Bin -ErrorAction Stop | Out-Null }
catch { Copy-Item $Bin $exe -Force }

Write-Output ("ARM {0} bin={1} size={2} launches={3}" -f $Tag, $Bin, (Get-Item $Bin).Length, $N)

# -- open N windows ---------------------------------------------------------
for ($i = 0; $i -lt $N; $i++) {
    $f = $files[$i]
    Start-Process -FilePath $exe -ArgumentList ('"' + $f.FullName + '"') -WorkingDirectory $exeRoot | Out-Null
    $want = $i + 1
    $deadline = (Get-Date).AddMilliseconds($WaitWindowMs)
    while ((Get-Date) -lt $deadline) {
        Start-Sleep -Milliseconds 250
        if ((Get-Windows -Pids (Get-RupixPids)).Count -ge $want) { break }
    }
    Start-Sleep -Milliseconds 400
}
Start-Sleep -Milliseconds $SettleMs
$pids = Get-RupixPids
$rows = Get-Windows -Pids $pids
Write-Output ("OPENED windows={0}" -f $rows.Count)
if ($rows.Count -ne $N) { Write-Output ("RESULT FAIL opened={0} want={1}" -f $rows.Count, $N); }

# How many viewer windows sit ABOVE the cover window in the z-order.
# ZMap rows are "hwnd:index" plus one "cover:index"; lower index = higher on screen.
function Get-AboveCount {
    param([string]$Map)
    $cover = -1
    $idx = @()
    foreach ($p in ($Map -split ';')) {
        if ($p.Length -eq 0) { continue }
        $kv = $p -split ':'
        if ($kv[0] -eq 'cover') { $cover = [int]$kv[1] } else { $idx += [int]$kv[1] }
    }
    if ($cover -lt 0) { return -1 }
    return @($idx | Where-Object { $_ -lt $cover }).Count
}

# -- cover the viewers with an unrelated window -----------------------------
# The point of this arm: minimized windows are restored by Windows itself as soon as the
# process gets foreground (measured: the control arm with -Cover 0 goes 3/3 -> 0/3 either
# way). What the wake path really decides is the Z-ORDER - with N windows buried under
# another app, only the ones it explicitly raises come back to the front.
$coverHwnd = [IntPtr]::Zero
$aboveBefore = 0
$aboveAfter = 0
$iconicBefore = 0
$iconicAfter = 0
if ($Cover -eq 1) {
    Add-Type -AssemblyName System.Windows.Forms
    $script:coverForm = New-Object System.Windows.Forms.Form
    $script:coverForm.Text = 'WAKE-COVER'
    $script:coverForm.Width = 1000
    $script:coverForm.Height = 700
    $script:coverForm.StartPosition = 'CenterScreen'
    $script:coverForm.Show()
    $script:coverForm.Activate()
    [System.Windows.Forms.Application]::DoEvents()
    Start-Sleep -Milliseconds 300
    $coverHwnd = $script:coverForm.Handle
    [TW]::RaiseAndUnpin($coverHwnd)
    Start-Sleep -Milliseconds 500
    $zBefore = [TW]::ZMap($pids, $coverHwnd)
    $aboveBefore = Get-AboveCount -Map $zBefore
    Write-Output ("COVER hwnd={0} z_before={1}" -f $coverHwnd.ToInt64(), $zBefore)
    Write-Output ("ABOVE_BEFORE above={0} of={1}" -f $aboveBefore, $rows.Count)
} else {
    [TW]::MinimizeAll($pids)
    Start-Sleep -Milliseconds 800
    $iconicBefore = [TW]::IconicCount($pids)
    Write-Output ("MINIMIZED iconic={0}/{1}" -f $iconicBefore, $rows.Count)
    Write-Output ("MIN_DETAIL " + [TW]::IconicMap($pids))
}

# -- trigger the wake path: second launch with no file ----------------------
if ($SkipWake -ne 1) {
    Start-Process -FilePath $exe -WorkingDirectory $exeRoot | Out-Null
} else {
    Write-Output 'WAKE skipped=1 (control arm)'
}
$t0 = Get-Date
if ($Cover -eq 1) {
    # Wait until every viewer is above the cover; a fix that only raises one of them
    # never gets there, so the loop times out on the old build (that is the finding).
    while (((Get-Date) - $t0).TotalMilliseconds -lt $WaitWakeMs) {
        Start-Sleep -Milliseconds 300
        $aboveAfter = Get-AboveCount -Map ([TW]::ZMap($pids, $coverHwnd))
        if ($aboveAfter -eq $rows.Count) { break }
    }
    Start-Sleep -Milliseconds 500
    $zAfter = [TW]::ZMap($pids, $coverHwnd)
    $aboveAfter = Get-AboveCount -Map $zAfter
    Write-Output ("Z_AFTER " + $zAfter)
    Write-Output ("ABOVE_AFTER above={0} of={1} took_ms={2:N0}" -f $aboveAfter, $rows.Count, ((Get-Date) - $t0).TotalMilliseconds)
} else {
    $iconicAfter = $iconicBefore
    while (((Get-Date) - $t0).TotalMilliseconds -lt $WaitWakeMs) {
        Start-Sleep -Milliseconds 300
        $iconicAfter = [TW]::IconicCount($pids)
        if ($iconicAfter -eq 0) { break }
    }
    Start-Sleep -Milliseconds 600
    $iconicAfter = [TW]::IconicCount($pids)
    $restored = $rows.Count - $iconicAfter
    Write-Output ("WAKE restored={0}/{1} iconic_after={2} took_ms={3:N0}" -f $restored, $rows.Count, $iconicAfter, ((Get-Date) - $t0).TotalMilliseconds)
    Write-Output ("AFTER_DETAIL " + [TW]::IconicMap($pids))
}
Write-Output ("WINDOWS_AFTER window_count={0} pids={1}" -f (Get-Windows -Pids (Get-RupixPids)).Count, ((Get-RupixPids) -join ','))

# -- log evidence -----------------------------------------------------------
# "\u5524\u56de\u7a97\u53e3 N \u6247" = the host's "brought back N windows" line.
$log = Join-Path $exeRoot ('temp\rupix-' + (Get-Date -Format 'yyyy-MM-dd') + '.log')
$wakeN = -1
if (Test-Path $log) {
    $txt = Get-Content $log -Raw -Encoding UTF8
    $m = [regex]::Matches($txt, '\u5524\u56de\u7a97\u53e3 (\d+) \u6247')
    if ($m.Count -gt 0) { $wakeN = [int]$m[$m.Count - 1].Groups[1].Value }
}
Write-Output ("LOG wake_lines_n={0}" -f $wakeN)

if ($Cover -eq 1) {
    $ok = ($rows.Count -eq $N) -and ($aboveAfter -eq $N) -and ($wakeN -eq $N)
    $verdict = if ($ok) { 'PASS' } else { 'FAIL' }
    Write-Output ("RESULT {0} expected_above_cover={1} got={2}" -f $verdict, $N, $aboveAfter)
} else {
    $ok = ($rows.Count -eq $N) -and ($restored -eq $N) -and ($wakeN -eq $N)
    $verdict = if ($ok) { 'PASS' } else { 'FAIL' }
    Write-Output ("RESULT {0} expected_restored={1} got={2}" -f $verdict, $N, $restored)
}

# -- teardown ---------------------------------------------------------------
if ($null -ne $script:coverForm) {
    $script:coverForm.Close()
    $script:coverForm.Dispose()
    $script:coverForm = $null
}
$pids = Get-RupixPids
[TW]::CloseAll($pids)
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
