@echo off
call "%~dp0_env.bat"
cargo run -p client -- --connect-lightyear %ASTRO_ADDR% --ly-key %ASTRO_KEY% --world-id polygon --render-manifest assets/ss14/render-manifest.json %*
