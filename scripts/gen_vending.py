#!/usr/bin/env python3
"""Генерация content/prototypes/vending.ron из прототипов SS14 (vending_machines.yml и
Catalog/VendingMachines/Inventories/*.yml). Использование: gen_vending.py <корень SS14>."""
import sys, glob, yaml

class L(yaml.SafeLoader):
    pass

L.add_multi_constructor('!', lambda loader, suffix, node: None)
root = sys.argv[1]
packs = {}
for f in glob.glob(root + '/Resources/Prototypes/Catalog/VendingMachines/Inventories/*.yml'):
    for e in yaml.load(open(f), Loader=L) or []:
        if e.get('type') == 'vendingMachineInventory':
            packs[e['id']] = e.get('startingInventory') or {}
ents = {}
for e in yaml.load(open(root + '/Resources/Prototypes/Entities/Structures/Machines/vending_machines.yml'), Loader=L):
    if e.get('type') == 'entity':
        ents[e['id']] = e

def chain(i):
    out = []
    while i in ents:
        out.append(ents[i])
        p = ents[i].get('parent')
        i = p[0] if isinstance(p, list) else p
    return out  # от потомка к предку

def comp(i, name):
    merged = {}
    for e in reversed(chain(i)):
        for c in e.get('components', []):
            if c['type'] == name:
                merged.update(c)
    return merged

lines = [
    '// Торговые автоматы (Resources/Prototypes/Entities/Structures/Machines/vending_machines.yml, наборы',
    '// товаров Catalog/VendingMachines/Inventories). Файл создан scripts/gen_vending.py; не править руками.',
    '// `stock` — startingInventory набора; предметы без прототипа в Build пропускаются при загрузке, как в SS14.',
    '[',
]
n = 0
for i, e in sorted(ents.items()):
    if e.get('abstract'):
        continue
    v = comp(i, 'VendingMachine')
    pack = v.get('pack')
    if pack not in packs:
        continue
    s = comp(i, 'Sprite')
    rsi = s.get('sprite')
    layers = s.get('layers') or []
    state = layers[0].get('state') if layers else None
    if not rsi or not state:
        continue
    name = next((x['name'] for x in chain(i) if x.get('name')), 'vending machine').replace('"', '')
    ar = comp(i, 'AccessReader').get('access') or []
    access = ''
    if ar:
        access = ', "AccessReader": (access: [%s])' % ', '.join('[%s]' % ', '.join('"%s"' % t for t in s) for s in ar)
    delay = comp(i, 'VendingMachineEject').get('ejectDelay', 1.2)
    quality = v.get('initialStockQuality', 1.0)
    stock = ', '.join(f'{k}: {int(a)}' for k, a in sorted(packs[pack].items()))
    lines.append(
        f'    (id: "{i}", name: Some("{name}"), components: {{'
        f'"Sprite": (sprite: "{rsi}", state: "{state}"), '
        f'"VendingMachine": (pack: "{pack}", initialStockQuality: {float(quality)}, ejectDelay: {float(delay)}, stock: ({stock})), '
        f'"ApcPowerReceiver": (load: 200.0){access}}}),'
    )
    n += 1
lines.append(']')
open('content/prototypes/vending.ron', 'w').write('\n'.join(lines) + '\n')
print(n, 'автоматов')
