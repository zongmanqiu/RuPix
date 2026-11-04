param(
    [Parameter(Mandatory=$true)][string]$Exe,
    [Parameter(Mandatory=$true)][string]$Img,
    [Parameter(Mandatory=$true)][string]$Out,
    [int]$Dx = 70
)
# Live probe for the `drag_image` switch (ini key, default on):
#   1. control  - press '1' (Actual size). The captured pixels MUST change.
#                 Without this arm, "nothing moved" cannot be told apart from
#                 "PrintWindow returned a dead frame" or "posted messages ignored".
#   2. effect   - drag the LEFT button 70px inside the canvas: pixels change
#                 (the picture pans) while the WINDOW rect stays identical.
#   3. cleanup  - WM_CLOSE and confirm the window is gone (no stuck instance).
# Run it against a PRIVATE exe root (its own ini + temp beside the exe), e.g.
#   copy .build\rupix.exe to .temp\rupix-drag\, set RUPIX_DECODERS_DIR to
#   .build\decoders, and pass -Out to a .temp folder - nothing lands in the repo.
Add-Type -AssemblyName System.Drawing
$src = @"
using System;
using System.Runtime.InteropServices;
public static class DragProbe {
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out R r);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr dc, uint flags);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool IsWindow(IntPtr h);
  public struct R { public int Left, Top, Right, Bottom; }
}
"@
Add-Type -TypeDefinition $src

$p = Start-Process -FilePath $Exe -ArgumentList "`"$Img`"" -PassThru
$sw = [Diagnostics.Stopwatch]::StartNew()
$h = [IntPtr]::Zero
while ($sw.Elapsed.TotalSeconds -lt 20) {
    Start-Sleep -Milliseconds 200
    $q = Get-Process -Id $p.Id -ErrorAction SilentlyContinue
    if ($null -ne $q) { $h = $q.MainWindowHandle; if ($h -ne [IntPtr]::Zero) { break } }
}
if ($h -eq [IntPtr]::Zero) { Write-Host "NO-HWND"; exit 1 }
Start-Sleep -Milliseconds 3000

function Get-Rect {
    $r = New-Object DragProbe+R; [void][DragProbe]::GetWindowRect($h, [ref]$r)
    return @($r.Left, $r.Top, $r.Right, $r.Bottom)
}
function Shot([string]$name) {
    $r = Get-Rect; $w = $r[2] - $r[0]; $ht = $r[3] - $r[1]
    $bmp = New-Object Drawing.Bitmap($w, $ht)
    $g = [Drawing.Graphics]::FromImage($bmp); $hdc = $g.GetHdc()
    [void][DragProbe]::PrintWindow($h, $hdc, 2)
    $g.ReleaseHdc($hdc); $g.Dispose()
    $out = Join-Path $Out "$name.png"
    $bmp.Save($out, [Drawing.Imaging.ImageFormat]::Png); $bmp.Dispose()
    return $out
}
function Sum([string]$path) {
    $b = [IO.File]::ReadAllBytes($path); $t = 0L
    for ($i = 0; $i -lt $b.Length; $i++) { $t += $b[$i] }
    return $t
}

$s0 = Shot "drag-s0"; $r0 = Get-Rect
[void][DragProbe]::PostMessage($h, 0x0100, [IntPtr]0x31, [IntPtr]0x00010001)  # WM_KEYDOWN '1'
[void][DragProbe]::PostMessage($h, 0x0101, [IntPtr]0x31, [IntPtr]0xC0010001)  # WM_KEYUP
Start-Sleep -Milliseconds 800
$s1 = Shot "drag-s1"; $r1 = Get-Rect
$c = Get-Rect; $cx = [int](($c[2] - $c[0]) / 2); $cy = [int](($c[3] - $c[1]) / 2)
[void][DragProbe]::PostMessage($h, 0x0201, [IntPtr]1, ([IntPtr]($cy * 65536 + $cx)))
for ($i = 1; $i -le 14; $i++) {
    $x = $cx + [int]($Dx * $i / 14)
    [void][DragProbe]::PostMessage($h, 0x0200, [IntPtr]0, ([IntPtr]($cy * 65536 + $x)))
    Start-Sleep -Milliseconds 25
}
[void][DragProbe]::PostMessage($h, 0x0202, [IntPtr]0, ([IntPtr]($cy * 65536 + ($cx + $Dx))))
Start-Sleep -Milliseconds 800
$s2 = Shot "drag-s2"; $r2 = Get-Rect

$t0 = Sum $s0; $t1 = Sum $s1; $t2 = Sum $s2
Write-Host ("foreground-is-us = {0}" -f (([DragProbe]::GetForegroundWindow()) -eq $h))
Write-Host ("rect s0={0} s1={1} s2={2}" -f ($r0 -join ","), ($r1 -join ","), ($r2 -join ","))
Write-Host ("CONTROL keypress changed pixels = {0}  (t0={1} t1={2})" -f ($t0 -ne $t1), $t0, $t1)
Write-Host ("EFFECT  drag changed pixels     = {0}  (t1={1} t2={2})" -f ($t1 -ne $t2), $t1, $t2)
Write-Host ("WINDOW  moved during drag       = {0}   (drag_image=1 expects False)" -f (($r1 -join ",") -ne ($r2 -join ",")))
[void][DragProbe]::PostMessage($h, 0x0010, [IntPtr]0, [IntPtr]0)
Start-Sleep -Milliseconds 1200
Write-Host ("closed = {0}" -f (-not [DragProbe]::IsWindow($h)))
