@echo off
cd /d "%~dp0.."
if "%ASTRO_ADDR%"=="" set ASTRO_ADDR=127.0.0.1:7779
if "%ASTRO_KEY%"=="" (
  if not exist .astro-key powershell -NoProfile -Command "-join ((1..32) | ForEach-Object { '{0:x2}' -f (Get-Random -Maximum 256) })" > .astro-key
  set /p ASTRO_KEY=<.astro-key
)
