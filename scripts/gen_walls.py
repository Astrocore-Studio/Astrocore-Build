#!/usr/bin/env python3
"""Прочность стен SS14 → content/wall_hp.ron (создано scripts/gen_walls.py).

Для каждого прототипа стены, окна и решётки с карты: порог `Destructible` (наименьший
`DamageTrigger`) и наборы защиты `Damageable.damageModifierSet` (множители и вычеты).
Использование: gen_walls.py <корень SS14> <карта.yml>."""
import re
import sys
from ss14yaml import Protos

P = Protos(sys.argv[1])
placed = set(re.findall(r'^- proto: (\w+)$', open(sys.argv[2]).read(), re.M))


def is_wall(p):
    return p.startswith('Wall') or ('Window' in p and 'Windoor' not in p) or 'Grille' in p


def table(d):
    return ', '.join('"%s": %s' % (k, float(v)) for k, v in sorted((d or {}).items()))


lines = ['// Прочность стен, окон и решёток по прототипам. Создано scripts/gen_walls.py.', '[']
for i in sorted(placed):
    if not is_wall(i) or not P.get('entity', i) or P.is_abstract(i):
        continue
    c = P.entity_components(i)
    hp = None
    for t in (c.get('Destructible') or {}).get('thresholds') or []:
        d = (t.get('trigger') or {}).get('damage')
        if isinstance(d, (int, float)) and (hp is None or d < hp):
            hp = float(d)
    if hp is None:
        continue
    mod = P.get('damageModifierSet', (c.get('Damageable') or {}).get('damageModifierSet') or '') or {}
    lines.append('    (proto: "%s", hp: %s, coefficients: {%s}, flat: {%s}),' % (
        i, hp, table(mod.get('coefficients')), table(mod.get('flatReductions'))))
lines.append(']')
open('content/wall_hp.ron', 'w').write('\n'.join(lines) + '\n')
print(len(lines) - 3, 'прототипов стен')
