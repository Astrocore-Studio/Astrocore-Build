#!/usr/bin/env python3
"""Компьютеры карты (ComputerVisuals) → content/prototypes/computers.ron и склеенные спрайты
(создано scripts/gen_computers.py). Спрайт: рама + клавиатура + экран + клавиши из computers.rsi.
Работает только консоль выдачи ID (`IdCardConsole`); остальные — неподвижные объекты со спрайтом.
Использование: gen_computers.py <корень SS14> <карта.yml>."""
import json
import os
import re
import sys
from PIL import Image
from ss14yaml import Protos

ROOT = sys.argv[1]
P = Protos(ROOT)
placed = set(re.findall(r'^- proto: (\w+)$', open(sys.argv[2]).read(), re.M))
TEX = ROOT + '/Resources/Textures/'
RSI = 'Structures/Machines/computers.rsi'
OUT = 'assets/ss14/Textures/Structures/Machines/computers_generated'


def q(s):
    return json.dumps(s, ensure_ascii=False)


src = json.load(open(TEX + RSI + '/meta.json'))
size = (src['size']['x'], src['size']['y'])


def frame(state, direction=0):
    """Первый кадр направления `direction` (0 юг, 1 север, 2 восток, 3 запад) состояния `state`."""
    try:
        im = Image.open(TEX + RSI + '/' + state + '.png').convert('RGBA')
    except FileNotFoundError:
        return None
    st = next((x for x in src['states'] if x['name'] == state), {})
    dirs = st.get('directions', 1)
    frames = len(st['delays'][0]) if st.get('delays') else 1
    cols = im.width // size[0]
    cell = (direction if dirs == 4 else 0) * frames
    x, y = (cell % cols) * size[0], (cell // cols) * size[1]
    return im.crop((x, y, x + size[0], y + size[1]))


# Уровни, которые консоль умеет выдавать (`IdCardConsoleComponent.AccessLevels`).
CONSOLE_LEVELS = [
    'Armory', 'Atmospherics', 'Bar', 'Brig', 'Detective', 'Captain', 'Cargo', 'Chapel', 'Chemistry',
    'ChiefEngineer', 'ChiefMedicalOfficer', 'Command', 'Cryogenics', 'Engineering', 'External', 'GenpopEnter',
    'GenpopLeave', 'HeadOfPersonnel', 'HeadOfSecurity', 'Hydroponics', 'Janitor', 'Kitchen', 'Lawyer',
    'Maintenance', 'Medical', 'Quartermaster', 'Research', 'ResearchDirector', 'Salvage', 'Security', 'Service',
    'Theatre',
]

lines = [
    '// Компьютеры карты: склеенные спрайты computers.rsi. Создано scripts/gen_computers.py.',
    '[',
]
n = 0
for i in sorted(placed):
    if not P.get('entity', i) or P.is_abstract(i):
        continue
    c = P.entity_components(i)
    v = c.get('ComputerVisuals')
    if not v or 'Computer' not in c:
        continue
    # Лист 2×2 в порядке направлений S, N, E, W: из него клиент собирает брус консоли.
    out = Image.new('RGBA', (size[0] * 2, size[1] * 2))
    for d in range(4):
        cell = Image.new('RGBA', size)
        for key in ('stateFrame', 'stateKeyboard', 'stateScreen', 'stateKeys'):
            s = v.get(key)
            f = frame(s, d) if s else None
            if f is not None:
                cell.alpha_composite(f)
        out.paste(cell, ((d % 2) * size[0], (d // 2) * size[1]))
    d = '%s/%s.rsi' % (OUT, i)
    os.makedirs(d, exist_ok=True)
    out.save(d + '/screen.png')
    json.dump({'version': 1, 'license': src.get('license', 'CC-BY-SA-3.0'), 'copyright': src.get('copyright', 'SS14') + ' (склеено scripts/gen_computers.py)',
               'size': {'x': size[0], 'y': size[1]}, 'states': [{'name': 'screen', 'directions': 4}]}, open(d + '/meta.json', 'w'), indent=1)
    load = c.get('ApcPowerReceiver', {}).get('powerLoad')
    parts = ['"Sprite": (sprite: %s, state: "screen")' % q('Structures/Machines/computers_generated/%s.rsi' % i)]
    parts.append('"ApcPowerReceiver": (load: %s)' % float(load if load is not None else 200))
    if 'IdCardConsole' in c:
        parts.append('"IdCardConsole": (access: [%s])' % ', '.join(q(t) for t in (c['IdCardConsole'].get('accessLevels') or CONSOLE_LEVELS)))
    lines.append('    (id: %s, name: Some(%s), components: {%s}),' % (q(i), q(P.entity_name(i) or i), ', '.join(parts)))
    n += 1
lines.append(']')
open('content/prototypes/computers.ron', 'w').write('\n'.join(lines) + '\n')
print(n, 'компьютеров')
