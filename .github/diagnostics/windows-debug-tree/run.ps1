$ErrorActionPreference = 'Stop'

$source = Join-Path $PSScriptRoot 'probe.c'
$exe = Join-Path $PSScriptRoot 'probe.exe'
$gcc = 'C:\msys64\mingw64\bin\gcc.exe'
if (-not (Test-Path -LiteralPath $gcc)) {
    $command = Get-Command gcc.exe -ErrorAction SilentlyContinue
    if ($null -eq $command) { Write-Error 'No MinGW gcc.exe found'; exit 2 }
    $gcc = $command.Source
}

Write-Output "probe_source_sha256=$((Get-FileHash -Algorithm SHA256 -LiteralPath $source).Hash)"
Write-Output "os=$([System.Environment]::OSVersion.VersionString) arch=$env:PROCESSOR_ARCHITECTURE gcc=$gcc"
& $gcc -std=c11 -Wall -Wextra -Werror -municode -O0 -o $exe $source
if ($LASTEXITCODE -ne 0) { Write-Output "compile_exit=$LASTEXITCODE"; exit 2 }
Write-Output "probe_binary_sha256=$((Get-FileHash -Algorithm SHA256 -LiteralPath $exe).Hash)"

$failed = $false
foreach ($iteration in 1..5) {
    foreach ($mode in @('normal', 'nested')) {
        Write-Output "start iteration=$iteration mode=$mode"
        & $exe "--$mode"
        $status = $LASTEXITCODE
        Write-Output "finish iteration=$iteration mode=$mode exit=$status"
        if ($status -ne 0) { $failed = $true }
    }
}
if ($failed) { exit 2 }
exit 0
