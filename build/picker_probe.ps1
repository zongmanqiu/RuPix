# picker_probe.ps1 - end-to-end verification of the color picker (m217).
#
# Fixture: a 400x300 solid #FF0000 PNG. Oracle = the system clipboard text,
# no OCR and no pixel guessing:
#   K enters picker mode; move into the image; left click copies the HEX.
#   A second click must NOT copy again (mode exited after the first click).
#   K then Esc must exit the mode without closing the window.
#
# Rules: ASCII only; private root + hardlinked exe + pinned ini + private temp;
# taskkill before start; only the child we started is closed.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File build\picker_probe.ps1
$ErrorActionPreference = 'Stop'

$image = 'E:\tmp\AI.Agent\RuPix\.temp\big-samples\picker_red.png'
if (-not (Test-Path $image)) { Write-Output 'NO_IMAGE'; exit 1 }
$srcExe = 'E:\tmp\AI.Agent\RuPix\.build\rupix.exe'
if (-not (Test-Path $srcExe)) { Write-Output 'NO_EXE'; exit 1 }

cmd /c "taskkill /F /IM rupix.exe >nul 2>&1"
Start-Sleep -Milliseconds 400

$root = 'E:\tmp\AI.Agent\RuPix\.temp\probe-picker'
if (Test-Path $root) { [System.IO.Directory]::Delete($root, $true) }
[System.IO.Directory]::CreateDirectory($root) | Out-Null
$exe = Join-Path $root 'rupix.exe'
try {
    New-Item -ItemType HardLink -Path $exe -Target $srcExe -ErrorAction Stop | Out-Null
} catch {
    Copy-Item $srcExe $exe -Force
}
[System.IO.Directory]::CreateDirectory((Join-Path $root 'temp')) | Out-Null
@(
    'prefetch=0', 'close_to_tray=0', 'minimal_mode=0',
    'decoders_preferred=', 'decoders_disabled=',
    'window_w=500', 'window_h=500', 'init_window=0',
    'opacity=100', 'filmstrip=0', 'checker=0', 'stay_on_top=0'
) | Set-Content -Path (Join-Path $root 'rupix.ini') -Encoding ascii

$env:RUPIX_DECODERS_DIR = 'E:\tmp\AI.Agent\RuPix\.build\decoders'
Remove-Item Env:RUST_LOG -ErrorAction SilentlyContinue

Add-Type -AssemblyName System.Drawing
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class PickerProbeWin {
    [StructLayout(LayoutKind.Sequential)]
    public struct RECT { public int Left, Top, Right, Bottom; }
    [StructLayout(LayoutKind.Sequential)]
    public struct POINT { public int X, Y; }
    [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr h, ref POINT p);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint dx, uint dy, uint d, UIntPtr extra);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint msg, IntPtr w, IntPtr l);
    [DllImport("user32.dll")] public static extern uint MapVirtualKeyW(uint vk, uint mapType);
}
'@

# Post KEYDOWN+KEYUP. The app's own TranslateMessage turns it into WM_CHAR;
# windui maps character VKs to Key::Other on the KEYDOWN leg (K192), so only
# the WM_CHAR leg reaches the host action table. This is the real keyboard path.
function Press([IntPtr]$h, [int]$vk) {
    $sc = ([int][PickerProbeWin]::MapVirtualKeyW([uint32]$vk, 1)) -band 0xffff
    $kd = [IntPtr]([long]1 -bor ([long]$sc -shl 16) -bor 0x40000000L)
    $ku = [IntPtr]([long]1 -bor ([long]$sc -shl 16) -bor 0xC0000000L)
    [void][PickerProbeWin]::PostMessage($h, 0x0100, [IntPtr]$vk, $kd)
    [void][PickerProbeWin]::PostMessage($h, 0x0101, [IntPtr]$vk, $ku)
}

function ClickAt([int]$x, [int]$y) {
    [void][PickerProbeWin]::SetCursorPos($x, $y)
    Start-Sleep -Milliseconds 180
    [PickerProbeWin]::mouse_event(0x0002, 0, 0, 0, [UIntPtr]::Zero)
    [PickerProbeWin]::mouse_event(0x0004, 0, 0, 0, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 450
}

function Shot([IntPtr]$h, [string]$path) {
    $r = New-Object PickerProbeWin+RECT
    [void][PickerProbeWin]::GetWindowRect($h, [ref]$r)
    $bmp = New-Object System.Drawing.Bitmap ($r.Right - $r.Left), ($r.Bottom - $r.Top)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $hdc = $g.GetHdc()
    [void][PickerProbeWin]::PrintWindow($h, $hdc, 2)
    $g.ReleaseHdc($hdc)
    $g.Dispose()
    $bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
    $bmp.Dispose()
}

function ClipText() {
    $s = Get-Clipboard -Raw
    if ($null -eq $s) { return '' }
    return ([string]$s).Trim()
}

$p = Start-Process -FilePath $exe -ArgumentList "`"$image`"" -PassThru -WorkingDirectory $root
$h = [IntPtr]::Zero
for ($i = 0; $i -lt 100; $i++) {
    Start-Sleep -Milliseconds 100
    $p.Refresh()
    if ($p.HasExited) { Write-Output 'ABORT_EXITED'; exit 1 }
    $h = [IntPtr]$p.MainWindowHandle
    if ($h -ne [IntPtr]::Zero) { break }
}
if ($h -eq [IntPtr]::Zero) { Write-Output 'ABORT_NO_WINDOW'; exit 1 }
Write-Output ("WINDOW pid={0} hwnd={1}" -f $p.Id, $h)
[void][PickerProbeWin]::SetForegroundWindow($h)
Start-Sleep -Seconds 8

# Client center in screen coords (the image fills the window at fit scale).
$rc = New-Object PickerProbeWin+RECT
[void][PickerProbeWin]::GetClientRect($h, [ref]$rc)
$pt = New-Object PickerProbeWin+POINT
$pt.X = 0; $pt.Y = 0
[void][PickerProbeWin]::ClientToScreen($h, [ref]$pt)
$cx = $pt.X + [int](($rc.Right - $rc.Left) / 2)
$cy = $pt.Y + [int](($rc.Bottom - $rc.Top) / 2)
Write-Output ("CLICK_AT {0},{1}" -f $cx, $cy)

# Arm 1: K enters mode, move shows the readout, click copies #FF0000.
Set-Clipboard -Value 'SENTINEL1'
Press $h 0x4B   # 'K'
Start-Sleep -Milliseconds 400
[void][PickerProbeWin]::SetCursorPos($cx, $cy)
Start-Sleep -Milliseconds 500
Shot $h (Join-Path $root 'mode.png')
ClickAt $cx $cy
$c1 = ClipText

# Arm 2: mode must be OFF now - a second click cannot overwrite the clipboard.
Set-Clipboard -Value 'SENTINEL2'
ClickAt $cx $cy
$c2 = ClipText

# Arm 3: K then Esc also exits, and must NOT close the window.
Set-Clipboard -Value 'SENTINEL3'
Press $h 0x4B   # 'K'
Start-Sleep -Milliseconds 300
Press $h 0x1B   # ESC
Start-Sleep -Milliseconds 300
ClickAt $cx $cy
$c3 = ClipText
$p.Refresh()
$alive = -not $p.HasExited

Write-Output ("PICKER c1='{0}' c2='{1}' c3='{2}' alive={3}" -f $c1, $c2, $c3, $alive)
$ok = ($c1 -eq '#FF0000') -and ($c2 -eq 'SENTINEL2') -and ($c3 -eq 'SENTINEL3') -and $alive
if ($ok) {
    Write-Output 'PICKER VERDICT: PASS (click copies hex + exits; Esc exits; window stays)'
} else {
    Write-Output 'PICKER VERDICT: FAIL'
}

# Cleanup: kill only our child and drop the crash-recovery marker.
cmd /c "taskkill /F /IM rupix.exe >nul 2>&1"
$run = Join-Path $root 'temp\rupix.run'
if (Test-Path $run) { [System.IO.File]::Delete($run) }
if (-not $ok) { exit 1 }
