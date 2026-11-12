#!/usr/bin/env python3
"""Копирует звуки SS14 из списка в assets/ss14/Audio и пересобирает ATTRIBUTIONS.md по
attributions.yml источника. Файлы с лицензией не CC (Custom и т. п.) пропускаются.
Использование: sync_audio.py <корень SS14> (список — SOUNDS ниже и уже лежащие файлы)."""
import os
import shutil
import sys
import yaml

ROOT = sys.argv[1] + '/Resources/Audio/'
DEST = 'assets/ss14/Audio/'
SOUNDS = [
    'Machines/machine_vend.ogg', 'Machines/custom_deny.ogg', 'Machines/door_lock_on.ogg',
    'Machines/door_lock_off.ogg', 'Machines/disposalflush.ogg', 'Machines/airlock_deny.ogg',
    'Machines/machine_switch.ogg', 'Machines/id_swipe.ogg', 'Effects/closetopen.ogg',
    'Effects/closetclose.ogg', 'Items/hypospray.ogg', 'Items/Medical/healthscanner.ogg',
    'Effects/Chemistry/bubbles.ogg', 'Items/defib_charge.ogg', 'Items/defib_zap.ogg',
    'Items/defib_success.ogg', 'Items/defib_failed.ogg', 'Items/defib_safety_on.ogg',
]


def attribution(rel):
    d, f = os.path.split(rel)
    path = ROOT + d + '/attributions.yml'
    if not os.path.exists(path):
        return None
    for e in yaml.safe_load(open(path)) or []:
        if f in e.get('files', []):
            return e
    return None


existing = []
for dp, _, fs in os.walk(DEST):
    for f in fs:
        if f.endswith('.ogg'):
            existing.append(os.path.relpath(os.path.join(dp, f), DEST))
rows = []
skipped = []
for rel in sorted(set(SOUNDS) | set(existing)):
    src = ROOT + rel
    if not os.path.exists(src):
        if rel in SOUNDS:
            skipped.append((rel, 'нет в источнике'))
        continue
    a = attribution(rel)
    lic = (a or {}).get('license', '')
    if rel in SOUNDS and (not a or not lic.startswith(('CC0', 'CC-BY'))):
        skipped.append((rel, lic or 'нет записи'))
        continue
    os.makedirs(os.path.dirname(DEST + rel), exist_ok=True)
    shutil.copyfile(src, DEST + rel)
    rows.append((rel, lic or 'в attributions.yml источника записи нет', (a or {}).get('copyright', '—')))
with open(DEST + 'ATTRIBUTIONS.md', 'w') as o:
    o.write('# Звуки SS14\n\nФайлы скопированы без изменений из space-wizards/space-station-14 (`Resources/Audio`,\n'
            'коммит `910154a7a8fcc6e4fc24f7c5b4925e405d74cd3e`) скриптом `scripts/sync_audio.py`. Лицензия и авторство — по записям\n'
            '`attributions.yml` рядом с исходными файлами:\n\n| Файл | Лицензия | Авторство |\n| --- | --- | --- |\n')
    for rel, lic, c in rows:
        o.write('| `%s` | %s | %s |\n' % (rel, lic, c.replace('|', '/').replace('\n', ' ')))
    if skipped:
        o.write('\nНе взяты (лицензия не подходит или файла нет): %s.\n' % ', '.join('`%s` (%s)' % s for s in skipped))
print(len(rows), 'файлов; пропущено:', skipped)
