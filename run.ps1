# Build and run the DAW (release build, which is what audio work needs).
# Usage:  .\run.ps1            release build + run
#         .\run.ps1 -Dev       debug build + run (faster compile, not for audio)
#         .\run.ps1 -- <args>  pass extra arguments through to the app
param(
    [switch]$Dev,
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$AppArgs
)

$ErrorActionPreference = 'Stop'

# Terminals opened before rustup was installed lack cargo on PATH.
$cargoBin = Join-Path $env:USERPROFILE '.cargo\bin'
if (-not (Get-Command cargo -ErrorAction SilentlyContinue) -and (Test-Path $cargoBin)) {
    $env:Path = "$cargoBin;$env:Path"
}
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Write-Error 'cargo not found. Install Rust from https://rustup.rs (see README).'
}

# bindgen (ASIO bindings) needs libclang.
if (-not $env:LIBCLANG_PATH) {
    $llvm = 'C:\Program Files\LLVM\bin'
    if (Test-Path (Join-Path $llvm 'libclang.dll')) {
        $env:LIBCLANG_PATH = $llvm
    } else {
        Write-Warning 'LIBCLANG_PATH not set and LLVM not found; the build may fail (see README).'
    }
}

Set-Location $PSScriptRoot

$cargoArgs = @('run', '-p', 'daw-app')
if (-not $Dev) { $cargoArgs += '--release' }
if ($AppArgs) { $cargoArgs += '--'; $cargoArgs += $AppArgs }

& cargo @cargoArgs
exit $LASTEXITCODE
