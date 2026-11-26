@echo off
setlocal
cd /d "%~dp0" || exit /b 1
cargo play %*
exit /b %errorlevel%
