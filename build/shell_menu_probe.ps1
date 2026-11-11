# shell_menu_probe.ps1 - end-to-end check of the "system menu" right-click entry (m313).
#
# What it proves (and what no unit test can), in one run:
#   * right click -> "system menu" really pops the SHELL's context menu (a top-level
#     window of class #32768, owned by our process), not a windui menu;
#   * that menu is legible: text + icons + submenus (phase B / C shots);
#   * picking a verb really runs it: "copy file address" must put the picture's
#     full path on the clipboard (phase D prints it).
#
# Input is POSTED, not played back as real mouse events: this session's foreground
# window is the console, so a real right-click would land on whatever covers that
# pixel. PostMessage(WM_RBUTTONDOWN/UP) reaches windui's own dispatcher just the
# same and carries the click position in lParam (client coordinates). The two
# phases that act INSIDE the native popup use the real cursor instead - a popup
# menu tracks the mouse itself and would ignore posted messages.
#
# Phases (row coordinates are client pixels in the windui menu; the native menu
# rows are SCREEN y, so pass menu-top + row offset):
#   A (default): open RuPix, right-click the picture, save the window shot.
#   B (-RowY n): click the n-th row of OUR menu (92 = "system menu" as measured on
#      this machine), then grab the native popup by its own rect.
#   C (-SubY y): hover a submenu parent with the real cursor (e.g. 7-Zip).
#   D (-VerbY y): click a verb with the real cursor and print the clipboard.
#
# Rules (same as every other probe here): ASCII only; private root + hardlinked
# exe + pinned ini; taskkill before start; only the child we started is closed.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File build\shell_menu_probe.ps1
#   powershell -ExecutionPolicy Bypass -File build\shell_menu_probe.ps1 -RowY 92
#   powershell -ExecutionPolicy Bypass -File build\shell_menu_probe.ps1 -RowY 92 -SubY 673
#   powershell -ExecutionPolicy Bypass -File build\shell_menu_probe.ps1 -RowY 92 -VerbY 707
param(
    [int]$RowY = 0,
    [int]$SubY = 0,
    [int]$VerbY = 0
)
$ErrorActionPreference = 'Stop'

$image = 'E:\tmp\AI.Agent\RuPix\.temp\big-samples\pan_gradient.png'
if (-not (Test-Path $image)) { Write-Output 'NO_IMAGE'; exit 1 }
$srcExe = 'E:\tmp\AI.Agent\RuPix\.build\rupix.exe'
if (-not (Test-Path $srcExe)) { Write-Output 'NO_EXE'; exit 1 }

cmd /c "taskkill /F /IM rupix.exe >nul 2>&1"
Start-Sleep -Milliseconds 400

$root = 'E:\tmp\AI.Agent\RuPix\.temp\probe-shell-menu'
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
    'window_w=560', 'window_h=480', 'init_window=0',
    'opacity=100', 'filmstrip=0', 'checker=0', 'stay_on_top=0',
    'ui_theme=0'
) | Set-Content -Path (Join-Path $root 'rupix.ini') -Encoding ascii

$env:RUPIX_DECODERS_DIR = 'E:\tmp\AI.Agent\RuPix\.build\decoders'
$env:RUST_LOG = 'debug,fontdb=error'

Add-Type -AssemblyName System.Drawing
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class SMP {
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
    [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
    [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr h, ref POINT p);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint dx, uint dy, uint d, UIntPtr e);
    [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern bool BringWindowToTop(IntPtr h);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int cx, int cy, uint flags);
    [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr l);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
    public delegate bool EnumProc(IntPtr h, IntPtr l);
    public static IntPtr FindByPid(uint want) {
        IntPtr found = IntPtr.Zero;
        EnumWindows((h, l) => {
            uint pid;
            GetWindowThreadProcessId(h, out pid);
            if (pid == want && IsWindowVisible(h)) { found = h; return false; }
            return true;
        }, IntPtr.Zero);
        return found;
    }
    // Mouse lParam packing is client coordinates, 16 bits each.
    public static IntPtr LP(int x, int y) { return (IntPtr)((y << 16) | (x & 0xFFFF)); }
    // The native popup menu is a top-level window of class "#32768". Finding it
    // by pid gives us its exact rect, so the screen grab needs no guessing.
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, System.Text.StringBuilder s, int n);
    public static string MenuWindowRect(uint pid) {        string found = "";
        EnumWindows((h, l) => {
            uint p;
            GetWindowThreadProcessId(h, out p);
            if (p != pid) { return true; }
            var sb = new System.Text.StringBuilder(64);
            GetClassNameW(h, sb, 64);
            RECT r;
            GetWindowRect(h, out r);
            found += sb.ToString() + "[" + r.Left + "," + r.Top + "," + r.Right + "," + r.Bottom + "] ";
            return true;
        }, IntPtr.Zero);
        return found;
    }
}
'@

function ShotWindow([IntPtr]$h, [string]$path) {
    $r = New-Object SMP+RECT
    [void][SMP]::GetWindowRect($h, [ref]$r)
    $bmp = New-Object System.Drawing.Bitmap ($r.Right - $r.Left), ($r.Bottom - $r.Top)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $hdc = $g.GetHdc()
    [void][SMP]::PrintWindow($h, $hdc, 2)
    $g.ReleaseHdc($hdc)
    $g.Dispose()
    $bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
    $bmp.Dispose()
    return @($r.Left, $r.Top, $r.Right, $r.Bottom)
}

function ShotScreen([int]$x, [int]$y, [int]$w, [int]$h, [string]$path) {
    $bmp = New-Object System.Drawing.Bitmap $w, $h
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen($x, $y, 0, 0, (New-Object System.Drawing.Size $w, $h))
    $g.Dispose()
    $bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
    $bmp.Dispose()
}

$proc = Start-Process -FilePath $exe -ArgumentList "`"$image`"" -PassThru
$h = [IntPtr]::Zero
for ($i = 0; $i -lt 60; $i++) {
    Start-Sleep -Milliseconds 200
    $h = [SMP]::FindByPid($proc.Id)
    if ($h -ne [IntPtr]::Zero) { break }
}
if ($h -eq [IntPtr]::Zero) { Write-Output 'NO_WINDOW'; exit 1 }
Start-Sleep -Milliseconds 1200

# Bring the window to the front. The native popup menu is refused (or flashes and
# closes) when its owner is not the foreground window, and this harness runs with
# somebody else's window in front. SetForegroundWindow is blocked by the
# foreground lock unless the caller "just received input" - a synthetic Alt tap
# is the classic way to unlock it.
[void][SMP]::ShowWindow($h, 9)
[void][SMP]::BringWindowToTop($h)
# HWND_TOPMOST (-1): there is a topmost game window in this session that would
# otherwise cover RuPix AND the native popup menu it owns.
[void][SMP]::SetWindowPos($h, [IntPtr](-1), 0, 0, 0, 0, 0x0003)
[SMP]::keybd_event(0x12, 0, 0, [UIntPtr]::Zero)
[SMP]::keybd_event(0x12, 0, 2, [UIntPtr]::Zero)
Start-Sleep -Milliseconds 200
[void][SMP]::SetForegroundWindow($h)
Start-Sleep -Milliseconds 400
$fg = ([SMP]::GetForegroundWindow() -eq $h)
Write-Output ("FG=" + $fg)

$cr = New-Object SMP+RECT
[void][SMP]::GetClientRect($h, [ref]$cr)
$cx = [int](($cr.Right - $cr.Left) / 2)
$cy = [int](($cr.Bottom - $cr.Top) / 2)
$sp = New-Object SMP+POINT
$sp.X = $cx
$sp.Y = $cy
[void][SMP]::ClientToScreen($h, [ref]$sp)
[void][SMP]::SetCursorPos($sp.X, $sp.Y)
Write-Output ("CLICK=" + $sp.X + "," + $sp.Y + " CLIENT=" + $cx + "," + $cy)
[void](ShotWindow $h (Join-Path $root 'shot_before.png'))

# Right click, posted with client coordinates (RDOWN=0x0204, RUP=0x0205).
[void][SMP]::PostMessage($h, 0x0204, [IntPtr]2, [SMP]::LP($cx, $cy))
Start-Sleep -Milliseconds 120
[void][SMP]::PostMessage($h, 0x0205, [IntPtr]0, [SMP]::LP($cx, $cy))
Start-Sleep -Milliseconds 600
[void](ShotWindow $h (Join-Path $root 'shot_menu.png'))
Write-Output ("MENU_SHOT=" + (Join-Path $root 'shot_menu.png'))

if ($RowY -gt 0) {
    # Hover then click the row (MOVE=0x0200, LDOWN=0x0201, LUP=0x0202).
    [void][SMP]::PostMessage($h, 0x0200, [IntPtr]0, [SMP]::LP(($cx + 40), $RowY))
    Start-Sleep -Milliseconds 300
    # Shot of the hover state: proves the row we are about to click is the one
    # that lights up (the menu may be clamped away from the click point).
    [void](ShotWindow $h (Join-Path $root 'shot_hover.png'))
    Write-Output ("HOVER_SHOT=" + (Join-Path $root 'shot_hover.png'))
    [void][SMP]::PostMessage($h, 0x0201, [IntPtr]1, [SMP]::LP(($cx + 40), $RowY))
    Start-Sleep -Milliseconds 120
    [void][SMP]::PostMessage($h, 0x0202, [IntPtr]0, [SMP]::LP(($cx + 40), $RowY))
    Start-Sleep -Milliseconds 1200
    $sx = [Math]::Max(0, $sp.X - 40)
    $sy = [Math]::Max(0, $sp.Y - 40)
    ShotScreen $sx $sy 780 880 (Join-Path $root 'shot_shell.png')
    ShotScreen 0 0 960 540 (Join-Path $root 'shot_full.png')
    $wins = [SMP]::MenuWindowRect($proc.Id)
    Write-Output ("WINDOWS=" + $wins)
    # Grab the native popup by its own rect (class #32768): the whole point is to
    # read its items, so a blind region grab is not good enough.
    if ($wins -match '#32768\[(-?\d+),(-?\d+),(-?\d+),(-?\d+)\]') {
        $mx = [int]$Matches[1]
        $my = [int]$Matches[2]
        $mw = [int]$Matches[3] - $mx
        $mh = [int]$Matches[4] - $my
        if ($mw -gt 0 -and $mh -gt 0) {
            ShotScreen $mx $my $mw $mh (Join-Path $root 'shot_native.png')
            Write-Output ("NATIVE_SHOT=" + (Join-Path $root 'shot_native.png') + " SIZE=" + $mw + "x" + $mh)
        }
        # Phase C: hover a row that owns a submenu, with the REAL cursor (a popup
        # menu tracks the mouse itself - posted messages would not reach it).
        # If IContextMenu::HandleMenuMsg were broken, the submenu would come up
        # empty; that is what this shot is for.
        if ($SubY -gt 0 -and $mw -gt 0) {
            [void][SMP]::SetCursorPos(($mx + 90), $SubY)
            Start-Sleep -Milliseconds 900
            ShotScreen ($mx - 10) ($SubY - 40) 700 520 (Join-Path $root 'shot_submenu.png')
            Write-Output ("SUBMENU_SHOT=" + (Join-Path $root 'shot_submenu.png'))
            [void][SMP]::SetCursorPos($sp.X, $sp.Y)
            Start-Sleep -Milliseconds 300
        }
        # Phase D: really click a verb (real mouse again) and read the clipboard
        # back. "Copy file path" is the cheapest verb whose effect can be read
        # without opening another program - it proves InvokeCommand ran.
        if ($VerbY -gt 0 -and $mw -gt 0) {
            [void][SMP]::SetCursorPos(($mx + 90), $VerbY)
            Start-Sleep -Milliseconds 600
            [SMP]::mouse_event(0x0002, 0, 0, 0, [UIntPtr]::Zero)
            [SMP]::mouse_event(0x0004, 0, 0, 0, [UIntPtr]::Zero)
            Start-Sleep -Milliseconds 1200
            ShotScreen ($mx - 10) ($my - 10) 700 560 (Join-Path $root 'shot_after_verb.png')
            Start-Sleep -Milliseconds 300
            $clip = ''
            try { $clip = Get-Clipboard -Raw -ErrorAction Stop } catch { $clip = 'CLIPBOARD_ERROR' }
            Write-Output ("CLIPBOARD=" + $clip)
        }
    }
    $r2 = New-Object SMP+RECT
    [void][SMP]::GetWindowRect($h, [ref]$r2)
    Write-Output ("RECT_AFTER=" + $r2.Left + "," + $r2.Top + "," + $r2.Right + "," + $r2.Bottom)
    Write-Output ("FULL_SHOT=" + (Join-Path $root 'shot_full.png'))
    Write-Output ("SHELL_SHOT=" + (Join-Path $root 'shot_shell.png'))
    Write-Output ("SHELL_ORIGIN=" + $sx + "," + $sy)
    Start-Sleep -Milliseconds 300
} else {
    # Close the (windui) menu so the app is left clean.
    [void][SMP]::PostMessage($h, 0x0100, [IntPtr]0x1B, [IntPtr]0)
    [void][SMP]::PostMessage($h, 0x0101, [IntPtr]0x1B, [IntPtr]0)
    Start-Sleep -Milliseconds 400
}

Start-Sleep -Milliseconds 300
cmd /c "taskkill /F /IM rupix.exe >nul 2>&1"
Write-Output 'DONE'
