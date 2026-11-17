#!/usr/bin/env python3
"""Причёски и бороды человека → content/prototypes/customization.ron (создано scripts/gen_customization.py).

Читает `Entities/Mobs/Customization/Markings/human_hair.yml` и `human_facial_hair.yml`
(маркировки `Hair` и `FacialHair`) и пишет прототипы с компонентом `Marking`:
`(part, sprite, state)`. Использование: gen_customization.py <Resources/Prototypes>."""
import json
import os
import sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import yaml
from ss14yaml import Loader

root = sys.argv[1]
files = [
    ('Hair', 'human_hair.yml'),
    ('FacialHair', 'human_facial_hair.yml'),
]
lines = ['// Причёски и бороды человека. Создано scripts/gen_customization.py.', '[']
n = 0
for part, name in files:
    path = os.path.join(root, 'Entities/Mobs/Customization/Markings', name)
    for m in yaml.load(open(path, encoding="utf-8"), Loader=Loader):
        if m.get('type') != 'marking' or m.get('bodyPart') != part:
            continue
        sp = (m.get('sprites') or [{}])[0]
        lines.append('    (id: %s, components: {"Marking": (part: %s, sprite: %s, state: %s)}),' % (
            json.dumps(m['id']), json.dumps(part), json.dumps(sp.get('sprite', '')), json.dumps(sp.get('state', ''))))
        n += 1
lines.append(']')
open('content/prototypes/customization.ron', 'w', encoding='utf-8').write('\n'.join(lines) + '\n')
print(n, 'маркировок')
