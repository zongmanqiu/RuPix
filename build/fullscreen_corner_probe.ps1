param([string]$Bin = 'E:\tmp\AI.Agent\RuPix\.build\rupix.exe')

# Iron rule 1: clear the field before start (this kills any running RuPix).
taskkill /F /IM rupix.exe 2>$null | Out-Null
Start-Sleep -Milliseconds 400

Add-Type -TypeDefinition @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public static class CP {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr l);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint dx, uint dy, uint d, UIntPtr e);
  [DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr h, int a, out int v, int s);

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
  public static string TitleOf(IntPtr h) {
    StringBuilder sb = new StringBuilder(256);
    GetWindowTextW(h, sb, 256);
    return sb.ToString();
  }
  public static string RectOf(IntPtr h) {
    RECT r;
    GetWindowRect(h, out r);
    return r.Left + "," + r.Top + "," + r.Right + "," + r.Bottom;
  }
  // A real left-button drag on the title strip. Returns the (dx, dy) the window moved.
  public static string DragTitleStrip(IntPtr h) {
    RECT r;
    GetWindowRect(h, out r);
    int x = r.Left + 200;
    int y = r.Top + 10;
    SetForegroundWindow(h);
    SetCursorPos(x, y);
    System.Threading.Thread.Sleep(250);
    mouse_event(0x0002, 0, 0, 0, UIntPtr.Zero);
    for (int i = 1; i <= 8; i++) {
      SetCursorPos(x + i * 30, y + i * 18);
      System.Threading.Thread.Sleep(35);
    }
    mouse_event(0x0004, 0, 0, 0, UIntPtr.Zero);
    System.Threading.Thread.Sleep(700);
    RECT r2;
    GetWindowRect(h, out r2);
    return (r2.Left - r.Left) + "," + (r2.Top - r.Top);
  }
}
'@

$root = 'E:\tmp\AI.Agent\RuPix\.temp\corner-probe'
if (Test-Path $root) { Remove-Item -Recurse -Force $root }
New-Item -ItemType Directory -Force -Path $root | Out-Null
Copy-Item $Bin (Join-Path $root 'rupix.exe') -Force
@(
 'allow_multiple=1','init_window=0','close_to_tray=0','minimal_mode=0',
 'opacity=100','filmstrip=0','checker=0','stay_on_top=0','window_w=800','window_h=600'
) | Out-File -Encoding ascii (Join-Path $root 'rupix.ini')

$env:RUPIX_DECODERS_DIR = 'E:\tmp\AI.Agent\RuPix\.build\decoders'
$proc = Start-Process -FilePath (Join-Path $root 'rupix.exe') -WorkingDirectory $root -PassThru
Write-Output ("LAUNCH pid={0}" -f $proc.Id)

$h = [IntPtr]::Zero
$t0 = Get-Date
while (((Get-Date) - $t0).TotalMilliseconds -lt 15000) {
  $h = [CP]::FindByPid([uint32]$proc.Id)
  if ($h -ne [IntPtr]::Zero) { break }
  Start-Sleep -Milliseconds 200
}
if ($h -eq [IntPtr]::Zero) { Write-Output 'NO_WINDOW'; exit 2 }
Write-Output ("WINDOW hwnd={0} title='{1}'" -f $h, [CP]::TitleOf($h))
Start-Sleep -Milliseconds 900

function Get-Pref([IntPtr]$hw) {
  $v = 0
  $hr = [CP]::DwmGetWindowAttribute($hw, 33, [ref]$v, 4)
  if ($hr -ne 0) { return -1 }
  return $v
}
function Tap-F11([IntPtr]$hw) {
  [CP]::SetForegroundWindow($hw) | Out-Null
  Start-Sleep -Milliseconds 250
  [CP]::PostMessage($hw, 0x100, [IntPtr]0x7A, [IntPtr]0) | Out-Null
  [CP]::PostMessage($hw, 0x101, [IntPtr]0x7A, [IntPtr]0) | Out-Null
}

$p0 = Get-Pref $h
Write-Output ("PREF before_fullscreen={0} (expect 2=ROUND)" -f $p0)
Tap-F11 $h
Start-Sleep -Milliseconds 1600
$p1 = Get-Pref $h
Write-Output ("PREF in_fullscreen={0} (expect 1=DONOTROUND)" -f $p1)
Write-Output ("RECT in_fullscreen={0}" -f [CP]::RectOf($h))

# Diagnostic ONLY: this simulated mouse input does NOT reproduce the OS
# window-move loop (both old and new binaries read 0,0). The no-drag fix is
# pinned by the left_drag unit test instead. Kept to record the rect.
$drag = [CP]::DragTitleStrip($h)
Write-Output ("DRAG simulated_dx_dy={0} (diagnostic only)" -f $drag)
Write-Output ("RECT after_drag={0}" -f [CP]::RectOf($h))

Tap-F11 $h
Start-Sleep -Milliseconds 1600
$p2 = Get-Pref $h
Write-Output ("PREF after_exit={0} (expect 2=ROUND)" -f $p2)

$ok = ($p0 -eq 2) -and ($p1 -eq 1) -and ($p2 -eq 2)
if ($ok) { Write-Output ("RESULT PASS hwnd={0}" -f $h) }
else { Write-Output ("RESULT FAIL hwnd={0}" -f $h) }

# Iron rule 3: full teardown, including temp/rupix.run.
taskkill /F /IM rupix.exe 2>$null | Out-Null
Start-Sleep -Milliseconds 300
$run = Join-Path $root 'temp\rupix.run'
if (Test-Path $run) { Remove-Item -Force $run }
