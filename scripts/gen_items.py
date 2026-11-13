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


def _secs(v, default):
    if v is None:
        return default
    return float(v)


def seeds():
    out = set()
    for fn in ('jobs.ron', 'vending.ron'):
        t = open('content/prototypes/' + fn).read()
        for m in re.finditer(r'"(\w+)"', t):
            out.add(m.group(1))
        for m in re.finditer(r'\b([A-Z]\w+): \d+', t):
            out.add(m.group(1))
    for fn in ('spawners.ron', 'storage.ron', 'dispensers.ron'):
        if os.path.exists('content/prototypes/' + fn):
            out |= set(re.findall(r'"(\w+)"', open('content/prototypes/' + fn).read()))
    t = open(MAP).read()
    out |= set(re.findall(r'^- proto: (\w+)$', t, re.M))
    # Результаты ХимМастера и все предметы-смесители.
    out |= {'Pill', 'ChemistryEmptyBottle01', 'ChemistryEmptyBottle02', 'ChemistryEmptyBottle03',
            'ChemistryEmptyBottle04', 'Defibrillator', 'DefibrillatorCompact'}
    for i in P.by_type['entity']:
        if not P.is_abstract(i) and 'ReactionMixer' in P.entity_components(i):
            out.add(i)
    return out


def fill_ids(comps):
    ids = []
    pda = comps.get('Pda', {}).get('id')
    if isinstance(pda, str):
        ids.append(pda)
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
    if isinstance(rsi, str):
        rsi = rsi.removeprefix('/Textures/').lstrip('/')
    if rsi and state:
        return rsi, state
    return None


def _wl_components():
    out = set()
    for e in P.by_type['entity'].values():
        for c in e.get('components') or []:
            if c.get('type') == 'Storage':
                for k in ('whitelist', 'blacklist'):
                    out |= set((c.get(k) or {}).get('components') or [])
    return out


WL_COMPONENTS = _wl_components()
GUN_SOUNDS = set()
PROJECTILES = set()
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
        extra = ''
        if isinstance(ms, str):
            extra += ', maxItemSize: %s' % q(ms)
        for key in ('whitelist', 'blacklist'):
            flt = st.get(key)
            if flt:
                extra += ', %s: (tags: [%s], components: [%s])' % (
                    key, ', '.join(q(t) for t in flt.get('tags') or []), ', '.join(q(t) for t in flt.get('components') or []))
        out.append('"Storage": (grid: [%s]%s)' % (grid, extra))
    pda = comps.get('Pda', {}).get('id')
    if isinstance(pda, str):
        out.append('"Pda": (card: %s)' % q(pda))
    if 'IdCard' in comps:
        tags = comps.get('Access', {}).get('tags') or []
        job = comps.get('PresetIdCard', {}).get('job')
        out.append('"IdCard": (access: [%s]%s)' % (', '.join(q(t) for t in tags), ', job: %s' % q(job) if isinstance(job, str) else ''))
    sol = comps.get('Solution')
    if sol:
        s = sol.get('solution') or {}
        rs = s.get('reagents') or []
        pairs = {}
        for r in rs:
            if isinstance(r, dict) and r.get('ReagentId'):
                pairs[r['ReagentId']] = pairs.get(r['ReagentId'], 0) + float(r.get('Quantity', 0))
        vol = s.get('maxVol')
        total = sum(pairs.values())
        out.append('"Solution": (maxVol: %s, canReact: %s, reagents: (%s))' % (float(vol) if vol is not None else max(total, 30.0), 'false' if s.get('canReact') is False else 'true', ', '.join('%s: %s' % (k, v) for k, v in sorted(pairs.items()))))
    ed = comps.get('Edible')
    if ed and sol:
        kind = ed.get('edible', 'Food')
        ta = ed.get('transferAmount')
        ta = 1000.0 if (kind == 'Pill' or (ta is None and 'transferAmount' in ed)) else float(ta if ta is not None else 5)
        out.append('"Edible": (transferAmount: %s, delay: %s, forceFeedDelay: %s, destroyOnEmpty: %s)' % (
            ta, float(ed.get('delay', 1.0)), float(ed.get('forceFeedDelay', 3.0)),
            'true' if ed.get('destroyOnEmpty', kind != 'Drink') else 'false'))
    st_ = comps.get('SolutionTransfer')
    if st_ is not None:
        out.append('"SolutionTransfer": (amount: %s, max: %s)' % (float(st_.get('transferAmount', 5)), float(st_.get('maxTransferAmount', 1000))))
    ha = comps.get('HealthAnalyzer')
    if ha is not None:
        out.append('"HealthAnalyzer": ()')
    if 'FitsInDispenser' in comps:
        out.append('"FitsInDispenser": ()')
    if 'RefillableSolution' in comps:
        out.append('"Refillable": ()')
    if 'DrainableSolution' in comps:
        out.append('"Drainable": ()')
    if 'MixableSolution' in comps:
        out.append('"MixableSolution": ()')
    rm = comps.get('ReactionMixer')
    if rm is not None:
        out.append('"ReactionMixer": (types: [%s], mixerType: %s, time: %s)' % (
            ', '.join(q(t) for t in rm.get('reactionTypes') or []), q(rm.get('mixerType', 'Machine')), float(_secs(rm.get('timeToMix'), 0.0))))
    sk = comps.get('Shakeable')
    if sk is not None:
        out.append('"Shakeable": (duration: %s, requireInHand: %s)' % (float(_secs(sk.get('shakeDuration'), 1.0)), 'true' if sk.get('requireInHand') else 'false'))
    if 'Pill' in comps:
        out.append('"Pill": ()')
    gun = comps.get('Gun')
    if gun is not None:
        sg = gun.get('soundGunshot')
        sound = (sg.get('path') if isinstance(sg, dict) else '') or ''
        sound = sound.removeprefix('/Audio/')
        mag = ''
        slot = ((comps.get('ItemSlots') or {}).get('slots') or {}).get('gun_magazine') or {}
        if 'ChamberMagazineAmmoProvider' in comps or 'MagazineAmmoProvider' in comps:
            mag = slot.get('startingItem') or ''
        prov = comps.get('BallisticAmmoProvider') or comps.get('RevolverAmmoProvider') or {}
        # Энергооружие: собственная батарея и стоимость выстрела (`BatteryAmmoProvider`).
        bat = comps.get('BatteryAmmoProvider') or {}
        cell = comps.get('Battery') or {}
        out.append('"Gun": (fireRate: %s, minAngle: %s, maxAngle: %s, sound: %s, magazine: %s, cartridge: %s, capacity: %d, auto: %s, shot: %s, fireCost: %s, charge: %s)' % (
            float(gun.get('fireRate', 1.0)), float(gun.get('minAngle', 2.0)), float(gun.get('maxAngle', 10.0)),
            q(sound), q(mag), q(prov.get('proto') or ''), int(prov.get('capacity', 0)),
            'true' if gun.get('selectedMode') == 'FullAuto' else 'false',
            q(bat.get('proto') or ''), float(bat.get('fireCost', 0)), float(cell.get('maxCharge', 0))))
        if bat.get('proto'):
            PROJECTILES.add(bat['proto'])
        if sound:
            GUN_SOUNDS.add(sound)
    elif 'BallisticAmmoProvider' in comps:
        bp = comps['BallisticAmmoProvider']
        if bp.get('proto'):
            out.append('"Magazine": (cartridge: %s, capacity: %d)' % (q(bp['proto']), int(bp.get('capacity', 0))))
    ca = comps.get('CartridgeAmmo')
    if ca is not None and ca.get('proto'):
        out.append('"Cartridge": (projectile: %s)' % q(ca['proto']))
    ex = comps.get('Explosive')
    if ex is not None:
        out.append('"Explosive": (explosionType: %s, totalIntensity: %s, intensitySlope: %s, maxIntensity: %s)' % (
            q(ex.get('explosionType', 'Default')), float(ex.get('totalIntensity', 10)),
            float(ex.get('intensitySlope', 1)), float(ex.get('maxIntensity', 4))))
    tt = comps.get('TimerTrigger')
    if tt is not None and 'TriggerOnUse' in comps:
        out.append('"TimerTrigger": (delay: %s)' % float(_secs(tt.get('delay'), 3.0)))
    mw = comps.get('MeleeWeapon')
    if mw is not None:
        # Урон по видам (`damage.types`); группы и режимы без прямого урона не переносятся.
        types = ((mw.get('damage') or {}).get('types') or {})
        types = {k: float(v) for k, v in types.items() if isinstance(v, (int, float)) and v}
        if types:
            out.append('"MeleeWeapon": (damage: (%s), attackRate: %s, bluntStaminaDamageFactor: %s)' % (
                ', '.join('%s: %s' % (k, v) for k, v in sorted(types.items())),
                float(mw.get('attackRate', 1.0)),
                float(mw.get('bluntStaminaDamageFactor', 0.5))))
    df = comps.get('Defibrillator')
    if df is not None:
        heal = (df.get('zapHeal') or {}).get('types') or {}
        draw = comps.get('PowerCellDraw') or {}
        charge = None
        if 'PowerCellDraw' in comps:
            slot = ((comps.get('ItemSlots') or {}).get('slots') or {}).get('cell_slot') or {}
            cell = slot.get('startingItem')
            charge = 0.0
            if cell:
                charge = float(((P.entity_components(cell).get('Battery')) or {}).get('startingCharge', 0))
        out.append('"Defibrillator": (zapHeal: {%s}, zapDamage: %s, writhe: %s, zapDelay: %s, doAfter: %s, useCharge: %s, drawRate: %s, charge: %s, powered: %s, toggled: %s)' % (
            ', '.join('%s: %s' % (q(k), float(v)) for k, v in sorted(heal.items())),
            float(df.get('zapDamage', 5)), 3.0, 5.0, float(_secs(df.get('doAfterDuration'), 3.0)),
            float(draw.get('useCharge', 0)), float(draw.get('drawRate', 1)),
            float(charge or 0.0), 'true' if charge is not None else 'false', 'true' if 'ToggleCellDraw' in comps else 'false'))
    inj = comps.get('Injector')
    if inj:
        modes = []
        for mid in inj.get('allowedModes') or []:
            mm = P.merged_proto('injectorMode', mid)
            modes.append('(id: %s, behavior: %s, amounts: [%s], mobTime: %s, selfModifier: %s, downedModifier: %s, delayPerVolume: %s, ignoreDelayForVolume: %s, drawTime: %s)' % (
                q(mid), q(mm.get('behavior', 'Inject')),
                ', '.join(str(float(a)) for a in mm.get('transferAmounts', [1, 5, 10, 15])),
                float(mm.get('mobTime', 5.0)), float(mm.get('selfModifier', 0.5)), float(mm.get('downedModifier', 0.5)),
                float(mm.get('delayPerVolume', 0.1)), float(mm.get('ignoreDelayForVolume', 5)), float(mm.get('containerDrawTime', 0.0))))
        active = inj.get('activeModeProtoId')
        idx = (inj.get('allowedModes') or []).index(active) if active in (inj.get('allowedModes') or []) else 0
        cur = inj.get('currentTransferAmount', 5)
        out.append('"Injector": (modes: [%s], active: %d, amount: %s)' % (', '.join(modes), idx, float(cur) if cur is not None else -1.0))
    tg = (comps.get('Tag') or {}).get('tags')
    if tg:
        out.append('"Tag": (tags: [%s])' % ', '.join(q(t) for t in tg))
    for cname in sorted(WL_COMPONENTS):
        if cname in comps and not any(o.startswith('"%s":' % cname) for o in out):
            out.append('"%s": ()' % cname)
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


def write_guns():
    """Снаряды патронов (`Projectile.damage`) и разброс дроби (`ProjectileSpread`) → guns.ron; звуки выстрелов."""
    out = ['// Снаряды SS14: урон выстрела и разброс дроби. Создано scripts/gen_items.py.', '[']
    done = set()
    todo = list(PROJECTILES)
    while todo:
        i = todo.pop()
        if i in done:
            continue
        done.add(i)
        c = P.entity_components(i)
        sp = c.get('ProjectileSpread')
        pr = c.get('Projectile')
        if sp:
            out.append('    (id: %s, components: {"ProjectileSpread": (projectile: %s, count: %d, spread: %s)}),' % (
                q(i), q(sp.get('proto', '')), int(sp.get('count', 1)), float(sp.get('spread', 0))))
            if sp.get('proto'):
                todo.append(sp['proto'])
        elif pr or c.get('HitscanBasicDamage'):
            src = pr or c.get('HitscanBasicDamage')
            types = ((src.get('damage') or {}).get('types') or {})
            types = {k: float(v) for k, v in types.items() if isinstance(v, (int, float))}
            stam = float((c.get('StaminaDamageOnCollide') or {}).get('damage', 0) or 0)
            out.append('    (id: %s, components: {"ProjectileDamage": (damage: (%s), stamina: %s)}),' % (
                q(i), ', '.join('%s: %s' % (k, v) for k, v in sorted(types.items())), stam))
    out.append(']')
    open('content/prototypes/guns.ron', 'w', encoding='utf-8').write('\n'.join(sorted(set(out[:2])) and out) + '\n')
    open('content/prototypes/gun_sounds.txt', 'w').write('\n'.join(sorted(GUN_SOUNDS)) + '\n')


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
        # Оружие тянет за собой магазин, патрон и снаряд (`ChamberMagazineAmmoProvider`, `CartridgeAmmo`).
        c = P.entity_components(i)
        if 'Gun' in c:
            slot = ((c.get('ItemSlots') or {}).get('slots') or {}).get('gun_magazine') or {}
            for dep in (slot.get('startingItem'), (c.get('BallisticAmmoProvider') or c.get('RevolverAmmoProvider') or {}).get('proto')):
                if dep:
                    todo.append(dep)
        if 'BallisticAmmoProvider' in c and 'Gun' not in c and (c['BallisticAmmoProvider'] or {}).get('proto'):
            todo.append(c['BallisticAmmoProvider']['proto'])
        if 'CartridgeAmmo' in c and c['CartridgeAmmo'].get('proto'):
            PROJECTILES.add(c['CartridgeAmmo']['proto'])
    lines.sort()
    head = [
        '// Предметы SS14, нужные должностям, автоматам, спавнерам и карте. Создано scripts/gen_items.py.',
        '[',
    ]
    open('content/prototypes/ss14_items.ron', 'w').write('\n'.join(head + [l for _, l in lines] + [']']) + '\n')
    write_guns()
    rsis = sorted({re.search(r'sprite: "([^"]+)"', l).group(1) for _, l in lines})
    open('/tmp/items_rsi.txt', 'w').write('\n'.join(rsis) + '\n')
    print(len(lines), 'предметов,', len(rsis), 'наборов RSI')


main()
