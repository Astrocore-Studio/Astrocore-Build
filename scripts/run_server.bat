@echo off
setlocal
cd /d "%~dp0.." || exit /b 1
cargo serve2d %*
exit /b %errorlevel%
