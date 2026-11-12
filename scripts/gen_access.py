#!/usr/bin/env python3
"""Доступ SS14: считыватели дверей и уровни должностей → content/door_access.ron (создано scripts/gen_access.py).

Для каждого прототипа двери из карты: списки доступа платы-электроники (`ContainerFill` → `board`
→ `AccessReader.access`) или собственный `AccessReader.access`. Использование:
gen_access.py <корень SS14> <карта.yml>."""
import json
import re
import sys
from ss14yaml import Protos

P = Protos(sys.argv[1])
placed = set(re.findall(r'^- proto: (\w+)$', open(sys.argv[2]).read(), re.M))


def lists_of(i):
    c = P.entity_components(i)
    ar = c.get('AccessReader')
    if not ar:
        return None
    if ar.get('access'):
        return ar['access']
    cf = c.get('ContainerFill', {}).get('containers', {})
    for b in cf.get('board', []):
        r = P.entity_components(b).get('AccessReader')
        if r and r.get('access'):
            return r['access']
    return None


lines = ['// Списки доступа считывателей по прототипам (двери). Создано scripts/gen_access.py.', '[']
n = 0
for i in sorted(placed):
    if not P.get('entity', i) or P.is_abstract(i):
        continue
    acc = lists_of(i)
    if not acc:
        continue
    sets = ', '.join('[%s]' % ', '.join(json.dumps(t) for t in s) for s in acc)
    lines.append('    (proto: %s, access: [%s]),' % (json.dumps(i), sets))
    n += 1
lines.append(']')
open('content/door_access.ron', 'w').write('\n'.join(lines) + '\n')
print(n, 'прототипов с доступом')
