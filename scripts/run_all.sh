#!/usr/bin/env bash
# Сервер и клиент вместе: сервер работает в фоне и останавливается вместе с клиентом.
. "$(dirname "$0")/_env.sh"
cargo build -p server -p client || exit 1
target/debug/server --listen "$ADDR" --key "$ASTRO_KEY" &
SERVER=$!
trap 'kill $SERVER 2>/dev/null' EXIT
sleep 1
target/debug/client --connect-lightyear "$ADDR" --ly-key "$ASTRO_KEY" \
  --world-id polygon --render-manifest assets/ss14/render-manifest.json "$@"
