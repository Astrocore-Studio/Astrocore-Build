#!/usr/bin/env python3
"""Типы взрывов (`ExplosionPrototype`) → content/prototypes/explosions.ron.

Для каждого `type: explosion` SS14: урон на единицу интенсивности (`damagePerIntensity.types`).
Использование: python3 scripts/gen_explosions.py <корень SS14>
"""
import sys

sys.path.insert(0, 'scripts')
from ss14yaml import Protos


def main():
    P = Protos(sys.argv[1])
    out = ['// Типы взрывов SS14 (explosion.yml): урон на единицу интенсивности. Генерируется scripts/gen_explosions.py.', '[']
    for i, e in sorted(P.by_type.get('explosion', {}).items()):
        types = ((e.get('damagePerIntensity') or {}).get('types') or {})
        types = {k: float(v) for k, v in types.items() if isinstance(v, (int, float))}
        out.append('    (id: "Explosion%s", components: {"ExplosionType": (damage: (%s))}),' % (
            i, ', '.join('%s: %s' % (k, v) for k, v in sorted(types.items()))))
    out.append(']')
    open('content/prototypes/explosions.ron', 'w', encoding='utf-8').write('\n'.join(out) + '\n')
    print('типов взрывов:', len(out) - 3)


if __name__ == '__main__':
    main()
