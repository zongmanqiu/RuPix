# crash_probe.ps1 - make the SHIPPED exe crash once and collect what it left behind.
#
# Why: examples/crash_probe only proves the example binary. The shipped rupix.exe is a
# different link, a different PDB pairing and a different exit path, so "the dump chain
# works" had never been shown for the artifact users actually run (m68).
#
# How the crash is induced WITHOUT adding a crash switch to the product: the host has a
# threat model where a third-party decoder DLL in decoders\ misbehaves. -DecodersDir points
# at a directory holding a test-only DLL (built from .temp\m68\crashplug) that starts a
# thread on load and writes to an unmapped address 1.2 s later. The host's sv_seh_call
# shim only wraps iv_* export calls, so a wild thread is exactly the part it does not
# cover - and exactly what a real bad DLL does.
#
# Rules honoured (same as flip_probe.ps1): ASCII only; private root with a hardlinked exe
# (a later redeploy cannot swap the bytes under the run, and the PDB pairing stays true);
# pinned ini (the shared .build\rupix.ini is hot - m60); private temp\ so the report and
# dump are ours alone; it never touches an instance it did not start.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File build\crash_probe.ps1 `
#       -DecodersDir <dir with crashplug.dll> -Image <some.png>
#
# Output: EXIT=... code=0x... wall=... then the files left in <root>\temp\.

param(
    [Parameter(Position = 0)][string]$Tag = 'm68',
    [Parameter(Position = 1)][string]$DecodersDir = 'E:\tmp\AI.Agent\RuPix\.temp\m68\decoders',
    [Parameter(Position = 2)][string]$Image = 'E:\tmp\AI.Agent\RuPix\.temp\m68\sample.png',
    [int]$WaitSeconds = 30
)
$ErrorActionPreference = 'Stop'

$build = 'E:\tmp\AI.Agent\RuPix\.build'
$srcExe = Join-Path $build 'rupix.exe'
if (-not (Test-Path $srcExe)) { Write-Output 'NO_EXE'; exit 1 }
if (-not (Test-Path $Image)) { Write-Output 'NO_IMAGE'; exit 1 }
if (-not (Test-Path (Join-Path $DecodersDir 'crashplug.dll'))) { Write-Output 'NO_FIXTURE_DLL'; exit 1 }

$root = "E:\tmp\AI.Agent\RuPix\.temp\probe-$Tag"
New-Item -ItemType Directory -Force -Path $root | Out-Null
$exe = Join-Path $root 'rupix.exe'
if (-not (Test-Path $exe)) {
    try {
        New-Item -ItemType HardLink -Path $exe -Target $srcExe -ErrorAction Stop | Out-Null
    } catch {
        Copy-Item $srcExe $exe -Force
    }
}
# Private diagnostics root: the report + dump must not mix with a real session's temp\.
Remove-Item -Recurse -Force (Join-Path $root 'temp') -ErrorAction SilentlyContinue
# Fixture scratch: stale markers from a previous arm would read as this run's evidence.
foreach ($m in @('fixture.txt', 'uef-hit.txt', 'veh.txt')) {
    Remove-Item -Force (Join-Path 'E:\tmp\AI.Agent\RuPix\.temp\m68' $m) -ErrorAction SilentlyContinue
}
@(
    'prefetch=0', 'close_to_tray=0', 'minimal_mode=0',
    'decoders_preferred=', 'decoders_disabled=',
    'window_w=800', 'window_h=600', 'init_window=0',
    'opacity=100', 'filmstrip=0', 'checker=0', 'stay_on_top=0'
) | Set-Content -Path (Join-Path $root 'rupix.ini') -Encoding ascii

# Only this child sees the fixture directory (a real decoder install would live in .build\decoders).
$env:RUPIX_DECODERS_DIR = $DecodersDir
# Shipped default log level: what a user's machine would leave on disk.
Remove-Item Env:RUST_LOG -ErrorAction SilentlyContinue
Remove-Item Env:RUPIX_DECODE_TRACE -ErrorAction SilentlyContinue

$t0 = Get-Date
$p = Start-Process -FilePath $exe -ArgumentList "`"$Image`"" -PassThru -WorkingDirectory $root
$exited = $false
for ($i = 0; $i -lt ($WaitSeconds * 10); $i++) {
    Start-Sleep -Milliseconds 100
    $p.Refresh()
    if ($p.HasExited) { $exited = $true; break }
}
$wall = ((Get-Date) - $t0).TotalMilliseconds
if (-not $exited) {
    # Our own probe only. Never used on an instance this script did not start.
    Stop-Process -Id $p.Id -Force
    Write-Output ("TIMEOUT after {0:N0} ms (pid {1} killed; fixture did not fire)" -f $wall, $p.Id)
    exit 1
}
$code = $p.ExitCode
Write-Output ("EXIT code=0x{0:X8} ({1}) wall={2:N0} ms root={3}" -f ($code -band 0xffffffff), $code, $wall, $root)

$temp = Join-Path $root 'temp'
if (-not (Test-Path $temp)) { Write-Output 'NO_TEMP_DIR'; exit 1 }
foreach ($f in Get-ChildItem $temp | Sort-Object Name) {
    Write-Output ("FILE {0} bytes={1}" -f $f.Name, $f.Length)
}
$crashLog = Get-ChildItem $temp -Filter 'crash-*.log' -ErrorAction SilentlyContinue |
    Sort-Object Name | Select-Object -Last 1
if ($null -ne $crashLog) {
    Write-Output ("--- {0} ---" -f $crashLog.Name)
    Get-Content $crashLog.FullName | Select-Object -First 40
} else {
    Write-Output 'NO_CRASH_LOG'
}
