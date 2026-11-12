#!/usr/bin/env python3
"""Спавнеры SS14 → content/prototypes/spawners.ron (создано scripts/gen_spawners.py).

Берутся спавнеры карты (`EntityTableSpawner`, `RandomSpawner`, `ConditionalSpawner`) и таблицы,
на которые они ссылаются (`NestedSelector`). Узлы таблиц: ent / group / all / nested с полями
`rolls`, `prob`, `weight`, `amount`. Использование: gen_spawners.py <корень SS14> <карта.yml>."""
import json
import re
import sys
from ss14yaml import Protos

P = Protos(sys.argv[1])
placed = set(re.findall(r'^- proto: (\w+)$', open(sys.argv[2]).read(), re.M))


def q(s):
    return json.dumps(s, ensure_ascii=False)


def num(v):
    """NumberSelector → ron: число или (min, max)."""
    if v is None:
        return None
    if isinstance(v, dict):
        if '__tag' in v and v['__tag'] == 'ConstantNumberSelector':
            return str(int(v['value']))
        if 'range' in v:
            v = v['range']
        else:
            return None
    if isinstance(v, (int, float)):
        return str(int(v))
    if isinstance(v, str) and ',' in v:
        a, b = [int(x) for x in v.split(',')]
        return '(min: %d, max: %d)' % (a, b)
    return str(int(v))


tables_needed = []


def node(n):
    tag = n.get('__tag') if isinstance(n, dict) else None
    f = []
    if tag in (None, 'EntSelector') and 'id' in n:
        f.append('k: "ent"')
        f.append('id: %s' % q(n['id']))
        a = num(n.get('amount'))
        if a is not None:
            f.append('amount: %s' % a)
    elif tag in ('GroupSelector', 'AllSelector'):
        f.append('k: "%s"' % ('group' if tag == 'GroupSelector' else 'all'))
        f.append('children: [%s]' % ', '.join(node(c) for c in n.get('children', [])))
    elif tag == 'NestedSelector':
        f.append('k: "nested"')
        f.append('table: %s' % q(n['tableId']))
        tables_needed.append(n['tableId'])
    elif tag == 'NoneSelector':
        f.append('k: "none"')
    else:
        raise SystemExit('неизвестный селектор %r' % (tag,))
    for key in ('weight', 'prob'):
        if key in n:
            f.append('%s: %s' % (key, float(n[key])))
    r = num(n.get('rolls'))
    if r is not None:
        f.append('rolls: %s' % r)
    return '(%s)' % ', '.join(f)


lines = [
    '// Спавнеры и таблицы сущностей SS14 (EntityTableSpawner, RandomSpawner, ConditionalSpawner).',
    '// Создано scripts/gen_spawners.py. Разбираются при старте раунда (spawners.rs).',
    '[',
]
done = set()
for p in sorted(placed):
    if not P.get('entity', p) or P.is_abstract(p):
        continue
    c = P.entity_components(p)
    if 'EntityTableSpawner' in c:
        s = c['EntityTableSpawner']
        body = 'offset: %s, table: %s' % (float(s.get('offset', 0.2)), node(s['table']))
    elif 'RandomSpawner' in c or 'ConditionalSpawner' in c:
        s = c.get('RandomSpawner') or c['ConditionalSpawner']
        protos = ', '.join(q(x) for x in s.get('prototypes', []))
        rare = ', '.join(q(x) for x in s.get('rarePrototypes', []))
        body = "offset: %s, chance: %s, rareChance: %s, prototypes: [%s], rarePrototypes: [%s]" % (
            float(s.get('offset', 0.2 if 'RandomSpawner' in c else 0.0)), float(s.get('chance', 1.0)), float(s.get('rareChance', 0.05)), protos, rare)
    else:
        continue
    lines.append('    (id: %s, components: {"Spawner": (%s)}),' % (q(p), body))
    done.add(p)
seen = set()
while tables_needed:
    t = tables_needed.pop()
    if t in seen:
        continue
    seen.add(t)
    tb = P.get('entityTable', t)
    if not tb:
        continue
    lines.append('    (id: %s, components: {"EntityTable": (table: %s)}),' % (q('Table' + t), node(tb['table'])))
lines.append(']')
open('content/prototypes/spawners.ron', 'w').write('\n'.join(lines) + '\n')
print(len(done), 'спавнеров,', len(seen), 'таблиц')
