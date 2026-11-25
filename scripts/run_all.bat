@echo off
setlocal
cd /d "%~dp0.." || exit /b 1
if not defined ASTRO_MAP set "ASTRO_MAP=content/maps/reach.ron"
if not defined CARGO_HOME set "CARGO_HOME=%CD%\.local\cargo"
if not defined CARGO_TARGET_DIR set "CARGO_TARGET_DIR=%CD%\.local\target"
if not defined CARGO_INCREMENTAL set "CARGO_INCREMENTAL=0"
if not exist "%ASTRO_MAP%" (
    echo Map not found: "%ASTRO_MAP%"
    exit /b 1
)
if not exist ".local\tmp" mkdir ".local\tmp" || exit /b 1
set "TEMP=%CD%\.local\tmp"
set "TMP=%TEMP%"
if not defined WGPU_BACKEND set "WGPU_BACKEND=vulkan"
cargo build --release --locked -p server -p client || exit /b 1
powershell.exe -NoProfile -ExecutionPolicy RemoteSigned -File "%~dp0run_saltern.ps1" -Mode All -Release -NoBuild -MapPath "%ASTRO_MAP%" -AdminAll Host -LogName mapping --no-shadows %*
exit /b %errorlevel%
