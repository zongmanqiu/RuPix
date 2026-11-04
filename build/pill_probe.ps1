# pill_probe.ps1 - sample the bottom pill area frame by frame to prove whether it
# flickers (visibility flips) or just shifts (width follows the status text).
#
# Why: user reports "bottom button bar keeps flashing" in minimal mode on an AVIF
# sequence (100 ms per frame). Two candidate causes look identical to the eye:
#   (a) hover_toolbar flips true/false every frame  -> the whole pill blinks
#   (b) the status cell width follows the frame number -> the centred pill slides
# This rig samples real pixels with PrintWindow so the answer is measured, not guessed.
#
# Rules honoured: ASCII only, private exe root with pinned ini (minimal_mode=1),
# touches only the instance it starts, closes with WM_CLOSE.

param(
    [Parameter(Position = 0)][string]$Image = 'E:\tmp\AI.Agent\RuPix\.build\1M\noise.avifs',
    [int]$Samples = 40,
    [int]$EveryMs = 30,
    [string]$Tag = 'pill',
    [int]$WindowW = 800,
    [int]$WindowH = 600
)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class PP {
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, int m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h, out RECT r);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L; public int T; public int R; public int B; }
}
"@

$build = 'E:\tmp\AI.Agent\RuPix\.build'
$srcExe = Join-Path $build 'rupix.exe'
$exeRoot = "E:\tmp\AI.Agent\RuPix\.temp\probe-$Tag"
New-Item -ItemType Directory -Force -Path $exeRoot | Out-Null
$exe = Join-Path $exeRoot 'rupix.exe'
if (-not (Test-Path $exe)) {
    try { New-Item -ItemType HardLink -Path $exe -Target $srcExe -ErrorAction Stop | Out-Null }
    catch { Copy-Item $srcExe $exe -Force }
}
$env:RUPIX_DECODERS_DIR = Join-Path $build 'decoders'
@(
    'prefetch=1', 'close_to_tray=0', 'minimal_mode=1',
    'decoders_preferred=', 'decoders_disabled=',
    ('window_w={0}' -f $WindowW), ('window_h={0}' -f $WindowH), 'init_window=0',
    'opacity=100', 'filmstrip=0', 'checker=0', 'stay_on_top=0'
) | Set-Content -Path (Join-Path $exeRoot 'rupix.ini') -Encoding ascii

$pre = @(Get-Process -Name rupix -ErrorAction SilentlyContinue)
if ($pre.Count -gt 0) { Write-Output 'ABORT_A_RUPIX_IS_RUNNING'; exit 1 }
if (-not (Test-Path $srcExe)) { Write-Output 'NO_EXE'; exit 1 }
if (-not (Test-Path $Image)) { Write-Output 'NO_IMAGE'; exit 1 }

$p = Start-Process -FilePath $exe -ArgumentList "`"$Image`"" -PassThru -WorkingDirectory $exeRoot
$h = [IntPtr]::Zero
for ($i = 0; $i -lt 60; $i++) {
    Start-Sleep -Milliseconds 200
    $p.Refresh()
    $h = $p.MainWindowHandle
    if ($h -ne [IntPtr]::Zero) { break }
}
if ($h -eq [IntPtr]::Zero) { Write-Output 'ABORT_NO_WINDOW'; exit 1 }
Start-Sleep -Seconds 3

$rc = [PP+RECT]::new()
[void][PP]::GetClientRect($h, [ref]$rc)
[int]$w = $rc.R - $rc.L
[int]$ht = $rc.B - $rc.T
Write-Output ("CLIENT {0}x{1}" -f $w, $ht)

try {
    $bmp = [System.Drawing.Bitmap]::new($w, $ht)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    [int]$yPill = $ht - 32
    [int]$cx = [int]($w / 2)
    [int[]]$xs = @(($cx - 200), ($cx - 100), $cx, ($cx + 100), ($cx + 200))
    Write-Output ("X = {0}" -f ($xs -join ','))

    $rows = New-Object System.Collections.ArrayList
    for ($i = 0; $i -lt $Samples; $i++) {
        $hdc = $g.GetHdc()
        [void][PP]::PrintWindow($h, $hdc, 2)
        $g.ReleaseHdc($hdc)
        $vals = New-Object System.Collections.ArrayList
        foreach ($x in $xs) {
            if ($x -lt 0 -or $x -ge $w) { [void]$vals.Add('--------'); continue }
            $argb = $bmp.GetPixel($x, $yPill).ToArgb()
            [void]$vals.Add(('{0:X8}' -f $argb))
        }
        [void]$rows.Add(('{0} {1}' -f $i, ($vals -join ' ')))
        Start-Sleep -Milliseconds $EveryMs
    }
    $out = "E:\tmp\AI.Agent\RuPix\.temp\pill_{0}.txt" -f $Tag
    $rows | Set-Content -Path $out -Encoding ascii
    Write-Output ("CAPTURED {0}" -f $out)
} catch {
    Write-Output ("FAILED {0}" -f $_.Exception.Message)
    Write-Output ($_.ScriptStackTrace)
}

[void][PP]::PostMessage($h, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero)
for ($i = 0; $i -lt 200; $i++) {
    Start-Sleep -Milliseconds 20
    $p.Refresh()
    if ($p.HasExited) { break }
}
Write-Output ("EXITED={0}" -f $p.HasExited)
