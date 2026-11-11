#!/usr/bin/env bash
# Сервер раунда Build. Дополнительные аргументы передаются серверу.
. "$(dirname "$0")/_env.sh"
exec cargo run -p server -- --listen "$ADDR" --key "$ASTRO_KEY" "$@"
