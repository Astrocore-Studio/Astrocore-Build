@echo off
call "%~dp0_env.bat"
cargo run -p server -- --listen %ASTRO_ADDR% --key %ASTRO_KEY% %*
