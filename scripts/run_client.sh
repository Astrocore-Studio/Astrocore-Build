#!/usr/bin/env bash
# Клиент Build. Дополнительные аргументы передаются клиенту (например, --third-person).
. "$(dirname "$0")/_env.sh"
exec cargo run -p client -- --connect-lightyear "$ADDR" --ly-key "$ASTRO_KEY" \
  --world-id polygon --render-manifest assets/ss14/render-manifest.json "$@"
