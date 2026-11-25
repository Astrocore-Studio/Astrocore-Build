@echo off
setlocal
cd /d "%~dp0" || exit /b 1
powershell.exe -NoProfile -ExecutionPolicy RemoteSigned -Command "& './scripts/run_saltern.ps1' -Mode All -Release -LocalEngine -GraphicsBackend Vulkan -RefreshGeneratedMap -MapPath '.local/maps/saltern-engineering.ron' -ClientArguments '--no-shadows'"
set "play_exit=%errorlevel%"
if not "%play_exit%"=="0" pause
exit /b %play_exit%
