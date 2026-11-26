@echo off
setlocal
cd /d "%~dp0.." || exit /b 1
cargo client2d %*
exit /b %errorlevel%
