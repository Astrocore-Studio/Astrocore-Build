# Process-local development paths. Dot-source from the Windows launch scripts.
$AstroBuildRoot = Split-Path -Parent $PSScriptRoot
$AstroLocalRoot = Join-Path $AstroBuildRoot '.local'
$AstroCargoBin = Join-Path $AstroLocalRoot 'cargo\bin'
if (Test-Path -LiteralPath (Join-Path $AstroCargoBin 'cargo.exe')) {
    if (-not $env:CARGO_HOME) { $env:CARGO_HOME = Join-Path $AstroLocalRoot 'cargo' }
    if (-not $env:RUSTUP_HOME) { $env:RUSTUP_HOME = Join-Path $AstroLocalRoot 'rustup' }
    $env:PATH = "$AstroCargoBin;$env:PATH"
}
$AstroMingwBin = Join-Path $AstroLocalRoot 'toolchain\mingw64\bin'
if (Test-Path -LiteralPath (Join-Path $AstroMingwBin 'gcc.exe')) {
    $env:PATH = "$AstroMingwBin;$env:PATH"
}
if (-not $env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR = Join-Path $AstroLocalRoot 'target' }
if (-not $env:CARGO_BUILD_JOBS) { $env:CARGO_BUILD_JOBS = '4' }
if (-not $env:CARGO_INCREMENTAL) { $env:CARGO_INCREMENTAL = '0' }
$AstroTempRoot = Join-Path $AstroLocalRoot 'tmp'
New-Item -ItemType Directory -Path $AstroTempRoot -Force | Out-Null
$env:TEMP = $AstroTempRoot
$env:TMP = $AstroTempRoot
$env:TMPDIR = $AstroTempRoot
$env:PYTHONUTF8 = '1'
$env:PYTHONDONTWRITEBYTECODE = '1'

