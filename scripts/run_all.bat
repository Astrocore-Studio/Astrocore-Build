@echo off
call "%~dp0_env.bat"
cargo build -p server -p client || exit /b 1
start "astrocore-server" target\debug\server.exe --listen %ASTRO_ADDR% --key %ASTRO_KEY%
timeout /t 1 >nul
target\debug\client.exe --connect-lightyear %ASTRO_ADDR% --ly-key %ASTRO_KEY% --world-id polygon --render-manifest assets/ss14/render-manifest.json %*
taskkill /im server.exe /f >nul 2>&1
