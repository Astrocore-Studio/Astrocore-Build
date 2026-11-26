#!/usr/bin/env bash
# Кладёт Astrocore Engine в соседний каталог ../Astrocore-Engine на ревизии из engine.rev.
# Build подключает Engine по пути; этот скрипт нужен после клонирования и после смены engine.rev.
set -euo pipefail
cd -- "$(dirname -- "$0")/.."
rev=$(tr -d '[:space:]' < engine.rev)
dir=../Astrocore-Engine
if [ ! -d "$dir/.git" ]; then
    git clone https://github.com/Astrocore-Studio/Astrocore-Engine.git "$dir"
fi
if [ -n "$(git -C "$dir" status --porcelain)" ]; then
    echo "В $dir есть незакоммиченные изменения: ревизию не меняю." >&2
    exit 1
fi
git -C "$dir" fetch --quiet origin
git -C "$dir" checkout --quiet "$rev"
echo "Astrocore Engine: $rev"
