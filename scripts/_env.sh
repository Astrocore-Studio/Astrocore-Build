# Общие настройки запуска. Ключ шифрования берётся из ASTRO_KEY или файла .astro-key
# (создаётся при первом запуске). Адрес — из ASTRO_ADDR.
cd "$(dirname "$0")/.." || exit 1
ADDR="${ASTRO_ADDR:-127.0.0.1:7779}"
if [ -z "${ASTRO_KEY:-}" ]; then
  [ -f .astro-key ] || head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n' > .astro-key
  ASTRO_KEY="$(cat .astro-key)"
fi
