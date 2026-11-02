# hang_probe.ps1 - run the SHIPPED exe against a decoder DLL that never returns,
# observe what the host says while the decode thread is stuck, then close our child.
#
# Why this exists (m71, todo doc section 18 item 6): the product has exactly one serial decode
# thread and no timeout cancellation (V6 breaker stays pinned). The only open
# question was whether a user can TELL that it is stuck. This probe makes the
# stuck state deterministic with a test-only decoder built from .temp\m71\hangplug
# (iv_probe claims .hng, iv_render sleeps 30 s), so "the hint fires" is measured,
# not assumed.
#
# Rules honoured (same as crash_probe.ps1): ASCII only; private root with a
# hardlinked exe; pinned ini; private temp\; only the child WE started is closed
# (WM_CLOSE first, own-pid Stop-Process as last resort).
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File build\hang_probe.ps1 `
#       -DecodersDir <dir with hangplug.dll> -Image <some .hng> -ObserveSeconds 12
#
# Output: OBSERVE=... state=... then the captured window (PNG) and the log tail.

param(
    [Parameter(Position = 0)][string]$Tag = 'm71',
    [Parameter(Position = 1)][string]$DecodersDir = 'E:\tmp\AI.Agent\RuPix\.temp\m71\decoders',
    [Parameter(Position = 2)][string]$Image = 'E:\tmp\AI.Agent\RuPix\.temp\m71\img\sample.hng',
    [int]$ObserveSeconds = 12,
    [string]$Shot = ''
)
$ErrorActionPreference = 'Stop'

$build = 'E:\tmp\AI.Agent\RuPix\.build'
$srcExe = Join-Path $build 'rupix.exe'
if (-not (Test-Path $srcExe)) { Write-Output 'NO_EXE'; exit 1 }
if (-not (Test-Path $Image)) { Write-Output 'NO_IMAGE'; exit 1 }
if (-not (Test-Path (Join-Path $DecodersDir 'hangplug.dll'))) { Write-Output 'NO_FIXTURE_DLL'; exit 1 }

$root = "E:\tmp\AI.Agent\RuPix\.temp\probe-hang-$Tag"
New-Item -ItemType Directory -Force -Path $root | Out-Null
$exe = Join-Path $root 'rupix.exe'
if (-not (Test-Path $exe)) {
    try {
        New-Item -ItemType HardLink -Path $exe -Target $srcExe -ErrorAction Stop | Out-Null
    } catch {
        Copy-Item $srcExe $exe -Force
    }
}
# Private diagnostics root: this run's log must not mix with a real session.
Remove-Item -Recurse -Force (Join-Path $root 'temp') -ErrorAction SilentlyContinue
@(
    'prefetch=0', 'close_to_tray=0', 'minimal_mode=0',
    'decoders_preferred=', 'decoders_disabled=',
    'window_w=800', 'window_h=600', 'init_window=0',
    'opacity=100', 'filmstrip=0', 'checker=0', 'stay_on_top=0'
) | Set-Content -Path (Join-Path $root 'rupix.ini') -Encoding ascii

$env:RUPIX_DECODERS_DIR = $DecodersDir
Remove-Item Env:RUST_LOG -ErrorAction SilentlyContinue
Remove-Item Env:RUPIX_DECODE_TRACE -ErrorAction SilentlyContinue

Add-Type -AssemblyName System.Drawing
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class HangProbeWin {
    [StructLayout(LayoutKind.Sequential)]
    public struct RECT { public int Left, Top, Right, Bottom; }
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint msg, IntPtr w, IntPtr l);
}
'@

$p = Start-Process -FilePath $exe -ArgumentList "`"$Image`"" -PassThru -WorkingDirectory $root
$t0 = Get-Date
$exited = $false
for ($i = 0; $i -lt ($ObserveSeconds * 10); $i++) {
    Start-Sleep -Milliseconds 100
    $p.Refresh()
    if ($p.HasExited) { $exited = $true; break }
}
$wall = ((Get-Date) - $t0).TotalMilliseconds
if ($exited) {
    Write-Output ("OBSERVE {0:N0} ms state=EXITED code={1}" -f $wall, $p.ExitCode)
} else {
    Write-Output ("OBSERVE {0:N0} ms state=ALIVE pid={1}" -f $wall, $p.Id)
    if ($Shot -ne '') {
        $p.Refresh()
        $h = $p.MainWindowHandle
        if ($h -ne [IntPtr]::Zero) {
            $r = New-Object HangProbeWin+RECT
            [void][HangProbeWin]::GetWindowRect($h, [ref]$r)
            Write-Output ("RECT {0},{1} {2}x{3}" -f $r.Left, $r.Top, ($r.Right - $r.Left), ($r.Bottom - $r.Top))
            $bmp = New-Object System.Drawing.Bitmap ($r.Right - $r.Left), ($r.Bottom - $r.Top)
            $g = [System.Drawing.Graphics]::FromImage($bmp)
            $hdc = $g.GetHdc()
            [void][HangProbeWin]::PrintWindow($h, $hdc, 2)
            $g.ReleaseHdc($hdc)
            $g.Dispose()
            $bmp.Save($Shot, [System.Drawing.Imaging.ImageFormat]::Png)
            $bmp.Dispose()
            Write-Output ("SHOT {0}" -f $Shot)
        } else {
            Write-Output 'NO_WINDOW'
        }
    }
}

Write-Output '--- log tail ---'
$logs = Get-ChildItem -Path (Join-Path $root 'temp') -Filter 'rupix-*.log' -ErrorAction SilentlyContinue
if ($logs) {
    Get-Content -Path $logs[0].FullName -Tail 25 -Encoding UTF8
} else {
    Write-Output 'NO_LOG'
}

if (-not $exited) {
    [void][HangProbeWin]::PostMessage($p.MainWindowHandle, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero)
    $closed = $false
    for ($i = 0; $i -lt 30; $i++) {
        Start-Sleep -Milliseconds 100
        $p.Refresh()
        if ($p.HasExited) { $closed = $true; break }
    }
    if (-not $closed) {
        # Our own probe child only; the decode thread is asleep inside the fixture.
        Stop-Process -Id $p.Id -Force
        Write-Output 'CLOSE forced (own pid)'
    } else {
        Write-Output 'CLOSE wm_close'
    }
}
