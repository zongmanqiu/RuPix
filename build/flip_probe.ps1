# flip_probe.ps1 - drive a RuPix instance through a fixed flip cadence and capture the log tail.
#
# Why: m58 needs the real numbers behind "flipping fast feels like one image per ~0.14 s".
# The log already carries the lines that settle it:
#   [decode-req]    one line PER REQUEST handed to the queue (kind/tier/seq/verdict)
#   [decode-trace]  one line PER ACTUAL DECODE (both above need RUPIX_DECODE_TRACE=1 + debug)
#   [main-latency]  one line per image that reached the screen: src=decode|prefetch + cost
# Counting decode-trace lines per file tells us whether one image gets decoded twice
# (prefetch in flight + Main sent anyway) and whether prefetch ever lands at all.
#
# -Level prod (m59) runs the arm the user actually gets: RUST_LOG / RUPIX_DECODE_TRACE are
# cleared, so the captured bytes are exactly what a shipped session leaves on disk - the only
# honest way to prove the default level is informative enough.
#
# Rules honoured: ASCII only (K78), only touches the instance it started, closes with
# WM_CLOSE so the session stamp is not left behind (K67), reads only the bytes it appended
# (the log is shared/rolling - m36 lesson).

param(
    [Parameter(Position = 0)][int]$Interval = 150,
    [Parameter(Position = 1)][int]$Presses = 24,
    [Parameter(Position = 2)][string]$Tag = 'fast',
    [Parameter(Position = 3)][string]$ImgDir = 'E:\tmp\AI.Agent\RuPix\.temp\m38-flip',
    [string]$Level = 'debug',
    [switch]$Private,
    [int]$Prefetch = 1,
    # m99: the queue-priority arm is only observable with the filmstrip running,
    # because that is what queues Thumb work against a Prefetch. Default keeps
    # every historical use of this rig byte-identical (filmstrip stays off).
    [switch]$Filmstrip,
    [switch]$CloseWhileBusy,
    # Arm identity includes the BINARY. Default is the shipped .build copy; an
    # attribution / experiment arm built into .temp passes its path here and must
    # be tagged differently. -Private hardlinks these exact bytes into its root.
    [string]$Exe = '',
    # m113: window size is part of arm identity (paint cost follows dst area).
    # Defaults keep every historical use of this rig byte-identical.
    [int]$WindowW = 800,
    [int]$WindowH = 600
)
$ErrorActionPreference = 'Stop'

Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class FP {
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, int m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern uint MapVirtualKeyW(uint vk, uint mapType);
}
"@

$build = 'E:\tmp\AI.Agent\RuPix\.build'
$srcExe = if ($Exe -eq '') { Join-Path $build 'rupix.exe' } else { $Exe }
$out = "E:\tmp\AI.Agent\RuPix\.temp\flip_{0}.log" -f $Tag
$exeRoot = $build

if ($Private) {
    # Arm identity includes the ENVIRONMENT. .build\rupix.ini is shared on this box and was
    # seen changing under a run (prefetch / close_to_tray / decoders_preferred), and the shared
    # temp\ log got replaced mid-series - so a -Private arm runs a hardlink of the SAME exe
    # bytes from a private root, with a pinned ini and its own temp\ log dir.
    # The hardlink also freezes the build: a later redeploy of .build\rupix.exe cannot swap
    # the bytes this arm measures (the session header in the log records which build it was).
    $exeRoot = "E:\tmp\AI.Agent\RuPix\.temp\probe-$Tag"
    New-Item -ItemType Directory -Force -Path $exeRoot | Out-Null
    $env:RUPIX_DECODERS_DIR = Join-Path $build 'decoders'
    @(
        "prefetch=$Prefetch", 'close_to_tray=0', 'minimal_mode=0',
        'decoders_preferred=', 'decoders_disabled=',
        ('window_w={0}' -f $WindowW), ('window_h={0}' -f $WindowH), 'init_window=0',
        'opacity=100', ("filmstrip={0}" -f $(if ($Filmstrip) { 1 } else { 0 })),
        'checker=0', 'stay_on_top=0'
    ) | Set-Content -Path (Join-Path $exeRoot 'rupix.ini') -Encoding ascii
}

$exe = Join-Path $exeRoot 'rupix.exe'
if ($Private -and -not (Test-Path $exe)) {
    try {
        New-Item -ItemType HardLink -Path $exe -Target $srcExe -ErrorAction Stop | Out-Null
    } catch {
        Copy-Item $srcExe $exe -Force
    }
}
$log = Join-Path $exeRoot ("temp\rupix-{0}.log" -f (Get-Date -Format 'yyyy-MM-dd'))

if (-not (Test-Path $srcExe)) { Write-Output 'NO_EXE'; exit 1 }
$pre = @(Get-Process -Name rupix -ErrorAction SilentlyContinue)
if ($pre.Count -gt 0) { Write-Output 'ABORT_A_RUPIX_IS_RUNNING'; exit 1 }
$first = Get-ChildItem $ImgDir -Include *.tif, *.jpg, *.png -File -Recurse | Sort-Object Name | Select-Object -First 1
if ($null -eq $first) { Write-Output 'NO_IMAGES'; exit 1 }

$preLen = 0L
if (Test-Path $log) { $preLen = (Get-Item $log).Length }

# Env applies to the child we start only. -Level prod clears both, so the shipped default
# filter (platform::logging::DEFAULT_FILTER) is what gets measured.
if ($Level -eq 'prod') {
    Remove-Item Env:RUST_LOG -ErrorAction SilentlyContinue
    Remove-Item Env:RUPIX_DECODE_TRACE -ErrorAction SilentlyContinue
} else {
    $env:RUST_LOG = 'debug,fontdb=error'
    $env:RUPIX_DECODE_TRACE = '1'
}

$p = Start-Process -FilePath $exe -ArgumentList "`"$($first.FullName)`"" -PassThru -WorkingDirectory $exeRoot
$h = [IntPtr]::Zero
for ($i = 0; $i -lt 60; $i++) {
    Start-Sleep -Milliseconds 200
    $p.Refresh()
    $h = $p.MainWindowHandle
    if ($h -ne [IntPtr]::Zero) { break }
}
if ($h -eq [IntPtr]::Zero) { Write-Output 'ABORT_NO_WINDOW'; exit 1 }

# Let the first image land before starting the cadence.
Start-Sleep -Seconds 8

$vk = 39  # VK_RIGHT
$sc = ([int][FP]::MapVirtualKeyW([uint32]$vk, 1)) -band 0xffff
$kd = [IntPtr]([long]1 -bor ([long]$sc -shl 16) -bor 0x40000000L)
$ku = [IntPtr]([long]1 -bor ([long]$sc -shl 16) -bor 0xC0000000L)
$t0 = Get-Date
for ($i = 0; $i -lt $Presses; $i++) {
    [void][FP]::PostMessage($h, 0x0100, [IntPtr]$vk, $kd)
    [void][FP]::PostMessage($h, 0x0101, [IntPtr]$vk, $ku)
    Start-Sleep -Milliseconds $Interval
}
$span = ((Get-Date) - $t0).TotalSeconds
Write-Output ("PRESSED {0} x{1} span={2:N1}s" -f $vk, $Presses, $span)

# Wait until the log stops growing (queue drained), 2 s samples, cap 180 s.
# -CloseWhileBusy skips this: the window then closes while the last decode is still in
# flight, which is the arm that measures "how long does exit take with work pending".
if (-not $CloseWhileBusy) {
    $last = -1L
    $still = 0
    for ($i = 0; $i -lt 90; $i++) {
        Start-Sleep -Seconds 2
        $cur = (Get-Item $log).Length
        if ($cur -eq $last) { $still++ } else { $still = 0; $last = $cur }
        if ($still -ge 3) { break }
    }
}

$fs = [System.IO.File]::Open($log, 'Open', 'Read', 'ReadWrite')
try {
    [void]$fs.Seek($preLen, [System.IO.SeekOrigin]::Begin)
    $sr = New-Object System.IO.StreamReader($fs)
    $text = $sr.ReadToEnd()
    $sr.Close()
} finally { $fs.Close() }
[System.IO.File]::WriteAllText($out, $text, (New-Object System.Text.UTF8Encoding($false)))
Write-Output ("CAPTURED {0} bytes={1} level={2}" -f $out, $text.Length, $Level)

$tClose = Get-Date
[void][FP]::PostMessage($h, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero)
# 20 ms steps: the 300 ms granularity of the old loop could only ever report ">=300 ms",
# which is useless for the question "how long does the process take to go away".
for ($i = 0; $i -lt 500; $i++) {
    Start-Sleep -Milliseconds 20
    $p.Refresh()
    if ($p.HasExited) { break }
}
$exitMs = ((Get-Date) - $tClose).TotalMilliseconds
Write-Output ("EXITED={0} exit_ms={1:N0} root={2}" -f $p.HasExited, $exitMs, $exeRoot)
