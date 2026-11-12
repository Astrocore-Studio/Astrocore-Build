#!/usr/bin/env python3
"""Предметы SS14 → content/prototypes/ss14_items.ron (создано scripts/gen_items.py; не править руками).

Берутся прототипы, нужные должностям (jobs.ron), торговым автоматам (vending.ron), спавнерам
(spawners.ron) и предметы карты (`Item`), плюс всё, что кладётся в них при создании
(`StorageFill`). Из компонентов переносятся Sprite, Item, Clothing, Tool, Storage; прототип без
спрайта пропускается. Использование: gen_items.py <корень SS14> <карта Maps/*.yml>."""
import json
import re
import sys
import os
from ss14yaml import Protos

ROOT = sys.argv[1]
MAP = sys.argv[2]
P = Protos(ROOT)
HAVE = set()  # уже описанные вручную прототипы Build
for f in os.listdir('content/prototypes'):
    if f in ('ss14_items.ron', 'spawners.ron'):
        continue
    HAVE |= set(re.findall(r'\(id: "(\w+)"', open('content/prototypes/' + f).read()))


def q(s):
    return json.dumps(s, ensure_ascii=False)


def seeds():
    out = set()
    for fn in ('jobs.ron', 'vending.ron'):
        t = open('content/prototypes/' + fn).read()
        for m in re.finditer(r'"(\w+)"', t):
            out.add(m.group(1))
        for m in re.finditer(r'\b([A-Z]\w+): \d+', t):
            out.add(m.group(1))
    if os.path.exists('content/prototypes/spawners.ron'):
        out |= set(re.findall(r'"(\w+)"', open('content/prototypes/spawners.ron').read()))
    t = open(MAP).read()
    out |= set(re.findall(r'^- proto: (\w+)$', t, re.M))
    return out


def fill_ids(comps):
    ids = []
    for c in comps.get('StorageFill', {}).get('contents') or []:
        if isinstance(c, dict) and c.get('id'):
            ids.append(c['id'])
    return ids


def sprite_of(comps):
    s = comps.get('Sprite')
    if not s:
        return None
    rsi = s.get('sprite')
    state = None
    # Слои важнее верхнего `state` (так выбирает SS14: слои перекрывают общий спрайт).
    for layer in s.get('layers') or []:
        if isinstance(layer, dict) and layer.get('state') and layer.get('visible', True):
            state = layer['state']
            rsi = layer.get('sprite', rsi)
            break
    if state is None:
        state = s.get('state')
    if rsi and state:
        return rsi, state
    return None


NOT_ITEMS = {'MobState', 'Mind', 'Body', 'Humanoid', 'Hands', 'Physics'}


def convert(i):
    if P.is_abstract(i) or not P.get('entity', i):
        return None
    comps = P.entity_components(i)
    if 'Item' not in comps and 'Clothing' not in comps:
        return None
    if 'MobState' in comps or 'Humanoid' in comps:
        return None
    sp = sprite_of(comps)
    # Спрайты мобов (призраки и т. п.) не предметы Build.
    if not sp or sp[0].startswith('Mobs/'):
        return None
    out = ['"Sprite": (sprite: %s, state: %s)' % (q(sp[0]), q(sp[1]))]
    item = comps.get('Item')
    size = (item or {}).get('size')
    out.append('"Item": (size: %s)' % q(size if isinstance(size, str) else 'Small'))
    cl = comps.get('Clothing')
    if cl:
        slots = cl.get('slots') or []
        slots = [str(s).lower() for s in (slots if isinstance(slots, list) else [slots])]
        if slots:
            out.append('"Clothing": (slots: [%s])' % ', '.join(q(s) for s in slots))
    tool = comps.get('Tool')
    if tool and tool.get('qualities'):
        qs = tool['qualities']
        qs = qs if isinstance(qs, list) else [qs]
        out.append('"Tool": (qualities: [%s])' % ', '.join(q(x) for x in qs))
    st = comps.get('Storage')
    if st and st.get('grid'):
        grid = ', '.join(q(g) for g in st['grid'])
        ms = st.get('maxItemSize')
        out.append('"Storage": (grid: [%s]%s)' % (grid, ', maxItemSize: %s' % q(ms) if isinstance(ms, str) else ''))
    fill = comps.get('StorageFill', {}).get('contents') or []
    ents = []
    for c in fill:
        if isinstance(c, dict) and c.get('id'):
            ents.append('(id: %s, prob: %s, orGroup: %s, amount: %d, maxAmount: %d)' % (
                q(c['id']), float(c.get('prob', 1)), q(c.get('orGroup', '')), int(c.get('amount', 1)), int(c.get('maxAmount', 1))))
    if ents:
        out.append('"StorageFill": (contents: [%s])' % ', '.join(ents))
    name = P.entity_name(i) or i
    return '    (id: %s, name: Some(%s), components: {%s}),' % (q(i), q(name), ', '.join(out))


def main():
    todo = list(seeds())
    seen = set()
    lines = []
    while todo:
        i = todo.pop()
        if i in seen or i in HAVE:
            continue
        seen.add(i)
        line = convert(i)
        if not line:
            continue
        lines.append((i, line))
        todo.extend(fill_ids(P.entity_components(i)))
    lines.sort()
    head = [
        '// Предметы SS14, нужные должностям, автоматам, спавнерам и карте. Создано scripts/gen_items.py.',
        '[',
    ]
    open('content/prototypes/ss14_items.ron', 'w').write('\n'.join(head + [l for _, l in lines] + [']']) + '\n')
    rsis = sorted({re.search(r'sprite: "([^"]+)"', l).group(1) for _, l in lines})
    open('/tmp/items_rsi.txt', 'w').write('\n'.join(rsis) + '\n')
    print(len(lines), 'предметов,', len(rsis), 'наборов RSI')


main()
