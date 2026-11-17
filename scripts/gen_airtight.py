#!/usr/bin/env python3
"""Воздухонепроницаемые сущности SS14 → content/prototypes/airtight.ron (создано scripts/gen_airtight.py).

Берёт прототипы, поставленные на указанных картах, у которых есть компонент `Airtight`
(`airBlockedDirection`, по умолчанию все стороны), кроме стен, окон и дверей: их импортёр
обрабатывает сам. Использование: gen_airtight.py <корень SS14> <карта.yml> [<карта.yml>...]."""
import json
import re
import sys
from ss14yaml import Protos

P = Protos(sys.argv[1])
placed = set()
for path in sys.argv[2:]:
    placed |= set(re.findall(r'^- proto: (\w+)$', open(path).read(), re.M))

SKIP = ('Wall', 'Airlock', 'Door', 'Firelock', 'Windoor', 'Shutter')
ALL = ['South', 'East', 'North', 'West']
lines = ['// Воздухонепроницаемые сущности (компонент Airtight). Создано scripts/gen_airtight.py.', '[']
n = 0
for i in sorted(placed):
    if i.startswith(SKIP) or 'Window' in i or not P.get('entity', i) or P.is_abstract(i):
        continue
    air = P.entity_components(i).get('Airtight')
    if air is None:
        continue
    dirs = air.get('airBlockedDirection') or ALL
    dirs = [d for d in (dirs if isinstance(dirs, list) else [dirs]) if d in ALL] or ALL
    lines.append('    (id: %s, components: {"Airtight": (directions: [%s])}),' % (
        json.dumps(i), ', '.join(json.dumps(d) for d in dirs)))
    n += 1
lines.append(']')
open('content/prototypes/airtight.ron', 'w').write('\n'.join(lines) + '\n')
print(n, 'прототипов')
