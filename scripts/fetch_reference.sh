#!/usr/bin/env bash
# Кладёт исходники Space Station 14 и RobustToolbox в ../reference на ревизиях из docs/REFERENCE.md.
# Референс нужен для сверки поведения при переносе и как источник Resources/ при разработке.
set -euo pipefail
cd -- "$(dirname -- "$0")/.."
ss14=$(sed -n 's/^| Space Station 14 .*`\([0-9a-f]\{40\}\)`.*/\1/p' docs/REFERENCE.md)
robust=$(sed -n 's/^| RobustToolbox .*`\([0-9a-f]\{40\}\)`.*/\1/p' docs/REFERENCE.md)
[ -n "$ss14" ] && [ -n "$robust" ] || { echo "Не найдены ревизии в docs/REFERENCE.md" >&2; exit 1; }
dir=../reference/space-station-14
mkdir -p ../reference
if [ ! -d "$dir/.git" ]; then
    git init --quiet "$dir"
    git -C "$dir" remote add origin https://github.com/space-wizards/space-station-14.git
fi
git -C "$dir" fetch --quiet --depth 1 origin "$ss14"
git -C "$dir" checkout --quiet FETCH_HEAD
git -C "$dir" submodule update --quiet --init --depth 1 RobustToolbox
actual=$(git -C "$dir/RobustToolbox" rev-parse HEAD)
if [ "$actual" != "$robust" ]; then
    echo "RobustToolbox: ожидалась $robust, получена $actual — обновите docs/REFERENCE.md" >&2
    exit 1
fi
echo "Референс: SS14 $ss14, RobustToolbox $robust"
