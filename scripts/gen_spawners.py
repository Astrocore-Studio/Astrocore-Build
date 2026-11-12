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
# --- Шкафы, ящики и морги (EntityStorage) ---
import os
from PIL import Image

OUT_RSI = 'assets/ss14/Textures/Structures/Storage/generated'
TEX = sys.argv[1] + '/Resources/Textures/'
storage_lines = [
    '// Шкафы, ящики и морги карты (EntityStorage, Lock, AccessReader, EntityTableContainerFill).',
    '// Создано scripts/gen_spawners.py. Закрытый и открытый вид — два прототипа (`X` и `XOpen`) со склеенными спрайтами.',
    '[',
]


def frame(rsi, state):
    meta = json.load(open(TEX + rsi + '/meta.json'))
    w, h = meta['size']['x'], meta['size']['y']
    im = Image.open(TEX + rsi + '/' + state + '.png').convert('RGBA')
    return im.crop((0, 0, w, h)), meta


def compose(rsi, base, door):
    layers = []
    out = None
    for s in (base, door):
        if not s:
            continue
        try:
            f, meta = frame(rsi, s)
        except FileNotFoundError:
            continue
        if out is None:
            out = Image.new('RGBA', f.size)
        out.alpha_composite(f)
    return out


storage_done = []
for p in sorted(placed):
    if not P.get('entity', p) or P.is_abstract(p):
        continue
    c = P.entity_components(p)
    if 'EntityStorage' not in c or 'Sprite' not in c:
        continue
    es = c['EntityStorage']
    sp = c['Sprite']
    rsi = sp.get('sprite')
    if not rsi:
        continue
    vis = c.get('EntityStorageVisuals', {})
    layers = sp.get('layers') or []
    base_closed = vis.get('stateBaseClosed')
    if not base_closed:
        for l in layers:
            if isinstance(l, dict) and l.get('state') and not l.get('map') or (isinstance(l, dict) and 'enum.StorageVisualLayers.Base' in (l.get('map') or [])):
                base_closed = l['state']
                break
    base_open = vis.get('stateBaseOpen') or base_closed
    door_closed = vis.get('stateDoorClosed')
    door_open = vis.get('stateDoorOpen')
    if not door_closed:
        for l in layers:
            if isinstance(l, dict) and 'enum.StorageVisualLayers.Door' in (l.get('map') or []):
                door_closed = l.get('state')
    closed = compose(rsi, base_closed, door_closed)
    opened = compose(rsi, base_open, door_open)
    if closed is None:
        continue
    d = '%s/%s.rsi' % (OUT_RSI, p)
    os.makedirs(d, exist_ok=True)
    src = json.load(open(TEX + rsi + '/meta.json'))
    states = [{'name': 'closed'}]
    closed.save(d + '/closed.png')
    if opened is not None:
        opened.save(d + '/open.png')
        states.append({'name': 'open'})
    json.dump({'version': 1, 'license': src.get('license', 'CC-BY-SA-3.0'), 'copyright': src.get('copyright', 'SS14') + ' (склеено scripts/gen_spawners.py)',
               'size': {'x': closed.width, 'y': closed.height}, 'states': states}, open(d + '/meta.json', 'w'), indent=1)
    rsipath = 'Structures/Storage/generated/%s.rsi' % p
    lock = c.get('Lock')
    ar = c.get('AccessReader', {}).get('access') or []
    fill = (c.get('EntityTableContainerFill', {}).get('containers') or {}).get('entity_storage')
    parts = ['"EntityStorage": (capacity: %d, openProto: %s)' % (int(es.get('capacity', 30)), q(p + 'Open'))]
    if lock is not None:
        parts.append('"Lock": (locked: %s)' % ('true' if lock.get('locked', True) else 'false'))
    if ar:
        parts.append('"AccessReader": (access: [%s])' % ', '.join('[%s]' % ', '.join(q(t) for t in s) for s in ar))
    if fill:
        parts.append('"ContainerFill": (table: %s)' % node(fill))
    name = P.entity_name(p) or p
    storage_lines.append('    (id: %s, name: Some(%s), components: {"Sprite": (sprite: %s, state: "closed"), %s}),' % (q(p), q(name), q(rsipath), ', '.join(parts)))
    if opened is not None:
        storage_lines.append('    (id: %s, name: Some(%s), components: {"Sprite": (sprite: %s, state: "open"), "EntityStorageOpen": (closedProto: %s)}),' % (q(p + 'Open'), q(name), q(rsipath), q(p)))
    storage_done.append(p)
storage_lines.append(']')
open('content/prototypes/storage.ron', 'w').write('\n'.join(storage_lines) + '\n')
print(len(storage_done), 'шкафов и ящиков')

# --- Раздатчики реагентов (ReagentDispenser) ---
disp = [
    '// Раздатчики реагентов карты: банки в хранилище и слот мензурки. Создано scripts/gen_spawners.py.',
    '[',
]
disp_done = []
for p in sorted(placed):
    if not P.get('entity', p) or P.is_abstract(p):
        continue
    c = P.entity_components(p)
    if 'ReagentDispenser' not in c:
        continue
    sp = c.get('Sprite', {})
    state = sp.get('state')
    for l in sp.get('layers') or []:
        if state is None and isinstance(l, dict) and l.get('state'):
            state = l['state']
    fill = (c.get('EntityTableContainerFill', {}).get('containers') or {}).get('storagebase')
    if not sp.get('sprite') or not state or not fill:
        continue
    rsi = sp['sprite'].removeprefix('/Textures/')
    disp.append('    (id: %s, name: Some(%s), components: {"Sprite": (sprite: %s, state: %s), "ApcPowerReceiver": (load: 200.0), "ReagentDispenser": (table: %s)}),' % (
        q(p), q(P.entity_name(p) or p), q(rsi), q(state), node(fill)))
    disp_done.append(p)
disp.append(']')
open('content/prototypes/dispensers.ron', 'w').write('\n'.join(disp) + '\n')
print(len(disp_done), 'раздатчиков:', disp_done)

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
