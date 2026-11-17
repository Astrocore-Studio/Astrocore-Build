#!/usr/bin/env python3
"""Копирует звуки SS14 из списка в assets/ss14/Audio и пересобирает ATTRIBUTIONS.md по
attributions.yml источника. Все исходные лицензии разрешены владельцем для переноса
2026-11-15 (docs/HANDOFF.md); исходные лицензии и авторство сохраняются.
Использование: sync_audio.py <корень SS14> (список — SOUNDS ниже и уже лежащие файлы)."""
import os
import shutil
import sys
import yaml

ROOT = sys.argv[1] + '/Resources/Audio/'
DEST = 'assets/ss14/Audio/'
SOUNDS = [
    'Machines/machine_vend.ogg', 'Machines/buzz-sigh.ogg', 'Machines/door_lock_on.ogg',
    'Machines/door_lock_off.ogg', 'Machines/disposalflush.ogg', 'Machines/airlock_deny.ogg',
    'Machines/machine_switch.ogg', 'Machines/id_swipe.ogg', 'Effects/closetopen.ogg',
    'Effects/closetclose.ogg', 'Items/hypospray.ogg', 'Items/Medical/healthscanner.ogg',
    'Effects/Chemistry/bubbles.ogg', 'Items/Defib/defib_charge.ogg', 'Items/Defib/defib_zap.ogg',
    'Items/Defib/defib_success.ogg', 'Items/Defib/defib_failed.ogg', 'Items/Defib/defib_safety_on.ogg',
    'Items/Defib/defib_safety_off.ogg', 'Machines/buzz_loop.ogg',
    'Items/soda_shake.ogg', 'Items/Medical/paper_centrifuge.ogg', 'Machines/spinning.ogg', 'Items/Defib/defib_ready.ogg',
]

# Звуки эмоций и рвоты из эффектов реагентов (`scripts/gen_chem.py`).
if os.path.exists('content/prototypes/reagent_sounds.txt'):
    SOUNDS += [l.strip() for l in open('content/prototypes/reagent_sounds.txt', encoding='utf8') if l.strip()]


# Звуки выстрелов (`scripts/gen_items.py`).
if os.path.exists('content/prototypes/gun_sounds.txt'):
    SOUNDS += [l.strip() for l in open('content/prototypes/gun_sounds.txt', encoding='utf8') if l.strip()]


def attribution(rel):
    d, f = os.path.split(rel)
    path = ROOT + d + '/attributions.yml'
    if not os.path.exists(path):
        return None
    for e in yaml.safe_load(open(path, encoding='utf8')) or []:
        if f in e.get('files', []):
            return e
    return None


existing = []
for dp, _, fs in os.walk(DEST):
    for f in fs:
        if f.endswith('.ogg'):
            existing.append(os.path.relpath(os.path.join(dp, f), DEST).replace(os.sep, '/'))
rows = []
skipped = []
for rel in sorted(set(SOUNDS) | set(existing)):
    src = ROOT + rel
    if not os.path.exists(src):
        if rel in SOUNDS:
            skipped.append((rel, 'нет в источнике'))
        continue
    a = attribution(rel)
    # Без записи действует лицензия Resources SS14 по умолчанию — CC-BY-SA-3.0 (как у спрайтов).
    lic = (a or {}).get('license', 'CC-BY-SA-3.0 (записи нет, лицензия Resources по умолчанию)')
    os.makedirs(os.path.dirname(DEST + rel), exist_ok=True)
    shutil.copyfile(src, DEST + rel)
    rows.append((rel, lic or 'в attributions.yml источника записи нет', (a or {}).get('copyright', 'авторство не указано'), (a or {}).get('source', '')))
with open(DEST + 'ATTRIBUTIONS.md', 'w', encoding='utf8') as o:
    o.write('# Звуки SS14\n\nФайлы скопированы без изменений из space-wizards/space-station-14 (`Resources/Audio`,\n'
            'снимок `31f782a809851c674bd8f86c3aca7c381118c539`) скриптом `scripts/sync_audio.py`. Лицензия, авторство и ссылка — по записям\n'
            '`attributions.yml` рядом с исходными файлами:\n\n| Файл | Лицензия | Авторство | Источник |\n| --- | --- | --- | --- |\n')
    for rel, lic, c, origin in rows:
        o.write('| `%s` | %s | %s | %s |\n' % (rel, lic, c.replace('|', '/').replace('\n', ' '), origin))
    if skipped:
        o.write('\nНе взяты (файла нет в источнике): %s.\n' % ', '.join('`%s` (%s)' % s for s in skipped))
print(len(rows), 'файлов; пропущено:', skipped)
