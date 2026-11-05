# decoder_inuse_probe.ps1 - what really happens when a decoder DLL is "uninstalled" while the
# session has already loaded it.
#
# Why: "uninstall a decoder while a picture is open" was on the multi-window risk list. It is NOT
# a multi-window problem (the settings page is a single app-wide window), so the right question is
# the general one: can the host move a DLL that it has already mapped into the process?
#
# Source facts this measures around:
#   - host.rs:41  "the host never FreeLibrarys mid-session" - uninstalling only flips Loaded::active=false,
#     the module stays mapped until the process exits.
#   - decoders_tab.rs:396  on failure the toast says, in effect, "the DLL is loaded; retry after
#     restarting RuPix" - that message is the product admitting exactly what this probe measures.
#
# So: launch with a .jpg (which loads common_*.dll), try to send that DLL to the RECYCLE BIN while
# the app runs, expect FAILURE; close the app, try again, expect SUCCESS.
#
# - A plain rename in place is NOT a valid stand-in: measured 2026-11-05, renaming a mapped DLL
#   succeeded (WHILE_RUNNING OK). The recycle bin is a move ACROSS directories (into $Recycle.Bin),
#   which needs delete access on the source - that is the operation that can fail. The first
#   version of this probe measured rename and produced a false "uninstall works" reading.
# - [Microsoft.VisualBasic.FileIO.FileSystem]::DeleteFile(..., SendToRecycleBin) is the shell
#   recycle path (SHFileOperation with FOF_ALLOWUNDO), i.e. the same one platform::recycle uses.
#   Side effect: a 2.3 MB copy lands in the bin; that is the price of testing the real path.
#
# ASCII only (K27/K78), private root + private decoder dir (K84/K146), WM_CLOSE teardown.

param(
    [Parameter(Position = 0)][string]$Tag = 'inuse',
    [string]$Image = 'E:\tmp\AI.Agent\RuPix\.build\1M\noise.jpg',
    [int]$SettleMs = 4000,
    [int]$WaitWindowMs = 25000
)
$ErrorActionPreference = 'Stop'

Add-Type @"
using System;
using System.Text;
using System.Collections.Generic;
using System.Runtime.InteropServices;
public static class IU {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr l);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, int m, IntPtr w, IntPtr l);
  public static int CountVisible(uint pid) {
    int n = 0;
    EnumWindows(delegate(IntPtr h, IntPtr l) {
      uint p; GetWindowThreadProcessId(h, out p);
      if (p == pid && IsWindowVisible(h)) n++;
      return true;
    }, IntPtr.Zero);
    return n;
  }
  public static void CloseAll(uint pid) {
    EnumWindows(delegate(IntPtr h, IntPtr l) {
      uint p; GetWindowThreadProcessId(h, out p);
      if (p == pid) PostMessage(h, 0x0010, IntPtr.Zero, IntPtr.Zero);
      return true;
    }, IntPtr.Zero);
  }
}
"@

$build = 'E:\tmp\AI.Agent\RuPix\.build'
$srcExe = Join-Path $build 'rupix.exe'
if (-not (Test-Path $srcExe)) { Write-Output 'NO_EXE'; exit 1 }
if (-not (Test-Path $Image)) { Write-Output ('NO_IMAGE ' + $Image); exit 1 }

$pre = @(Get-Process -Name rupix -ErrorAction SilentlyContinue)
if ($pre.Count -gt 0) { Write-Output 'ABORT_A_RUPIX_IS_RUNNING'; exit 1 }

$exeRoot = "E:\tmp\AI.Agent\RuPix\.temp\probe-inuse-$Tag"
New-Item -ItemType Directory -Force -Path $exeRoot | Out-Null
$decDir = Join-Path $exeRoot 'decoders'
New-Item -ItemType Directory -Force -Path $decDir | Out-Null

# Only one decoder in the private dir: keeps the arm fast and makes "which file" unambiguous.
$srcDll = @(Get-ChildItem (Join-Path $build 'decoders') -Filter 'common_*.dll' -File)[0]
$dll = Join-Path $decDir $srcDll.Name
if (-not (Test-Path $dll)) { Copy-Item $srcDll.FullName $dll -Force }
Write-Output ("DLL {0} bytes={1}" -f $srcDll.Name, (Get-Item $dll).Length)

$env:RUPIX_DECODERS_DIR = $decDir
$ini = @(
    'allow_multiple=0',
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

$proc = Start-Process -FilePath $exe -ArgumentList ('"' + $Image + '"') -PassThru -WorkingDirectory $exeRoot
$deadline = (Get-Date).AddMilliseconds($WaitWindowMs)
while ((Get-Date) -lt $deadline) {
    Start-Sleep -Milliseconds 250
    if ([IU]::CountVisible([uint32]$proc.Id) -ge 1) { break }
}
Start-Sleep -Milliseconds $SettleMs

Add-Type -AssemblyName Microsoft.VisualBasic

function Try-Recycle([string]$Path) {
    try {
        [Microsoft.VisualBasic.FileIO.FileSystem]::DeleteFile(
            $Path,
            [Microsoft.VisualBasic.FileIO.UIOption]::OnlyErrorDialogs,
            [Microsoft.VisualBasic.FileIO.RecycleOption]::SendToRecycleBin)
        if (Test-Path $Path) { return 'FAIL still-there-after-call' }
        return 'OK'
    } catch {
        return ('FAIL ' + $_.Exception.Message)
    }
}

$whileRunning = Try-Recycle $dll
$stillThere = Test-Path $dll
Write-Output ("WHILE_RUNNING {0} still_there={1}" -f $whileRunning, $stillThere)

# Whether the picture actually got decoded matters: an unloaded DLL is trivially movable.
$log = Join-Path $exeRoot ('temp\rupix-' + (Get-Date -Format 'yyyy-MM-dd') + '.log')
$latency = 0
if (Test-Path $log) {
    $txt = Get-Content $log -Raw -Encoding UTF8
    $latency = ([regex]::Matches($txt, '\[main-latency\]')).Count
}
Write-Output ("DECODED_LINES {0}" -f $latency)

[IU]::CloseAll([uint32]$proc.Id)
$t0 = Get-Date
$gone = $false
while (((Get-Date) - $t0).TotalMilliseconds -lt 15000) {
    Start-Sleep -Milliseconds 200
    if ($null -eq (Get-Process -Id $proc.Id -ErrorAction SilentlyContinue)) { $gone = $true; break }
}
Write-Output ("CLOSED clean={0} exit_ms={1:N0}" -f $gone, ((Get-Date) - $t0).TotalMilliseconds)
if (-not $gone) {
    Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
    Write-Output 'TEARDOWN force_killed=1'
}

$afterExit = if (-not (Test-Path $dll)) { 'ALREADY_MOVED' } else { Try-Recycle $dll }
Write-Output ("AFTER_EXIT {0} still_there={1}" -f $afterExit, (Test-Path $dll))

$ok = ($whileRunning -ne 'OK') -and ($afterExit -eq 'OK' -or $afterExit -eq 'ALREADY_MOVED')
$verdict = if ($ok) { 'PASS' } else { 'FAIL' }
Write-Output ("RESULT {0} (expect: while running NOT movable, after exit movable)" -f $verdict)

# -- cleanup ---------------------------------------------------------------
$marker = Join-Path $exeRoot 'temp\rupix.run'
if (Test-Path $marker) { Remove-Item $marker -Force -ErrorAction SilentlyContinue }
Write-Output 'DONE'
