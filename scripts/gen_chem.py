#!/usr/bin/env python3
"""Реагенты и реакции SS14 → content/prototypes/reagents.ron и reactions.ron (создано scripts/gen_chem.py).

Реагенты: название, цвет, метаболизм по стадиям (`Bloodstream`, `Digestion`, `Respiration`) с эффектами
`HealthChange`, `EvenHealthChange`, `DistributedHealthChange`, `Satiate`, `ModifyBleed`, `ModifyBloodLevel`,
`Oxygenate`, `ModifyLungGas`, `AdjustReagent` и условиями `ReagentCondition`, `MetabolizerTypeCondition`,
`MobStateCondition`, `TemperatureCondition`. Остальные эффекты (дрожь, рвота, эмоции…) отбрасываются, в отчёте
перечислены. Реакции: все, с `minTemp`/`maxTemp`, `conserveEnergy` и видами смесителя.
Использование: gen_chem.py <корень SS14>."""
import collections
import glob
import json
import re
import sys
from ss14yaml import Protos

P = Protos(sys.argv[1])
ROOT = sys.argv[1]


def q(s):
    return json.dumps(s, ensure_ascii=False)


def f(x):
    return repr(float(x))


names = {}
for fn in glob.glob(ROOT + '/Resources/Locale/en-US/reagents/**/*.ftl', recursive=True):
    for m in re.finditer(r'^(reagent-name-[\w-]+) = (.+)$', open(fn).read(), re.M):
        names[m.group(1)] = m.group(2)

GROUPS = {g['id']: g['damageTypes'] for g in P.by_type['damageGroup'].values()}
skipped = collections.Counter()
kept = collections.Counter()
ROOM = 293.15


def cond(c):
    t = c.get('__tag')
    if t == 'ReagentCondition':
        return '(c: "reagent", reagent: %s, min: %s, max: %s)' % (q(c['reagent']), f(c.get('min', 0)), f(c.get('max', -1)))
    if t == 'MetabolizerTypeCondition':
        types = c.get('type', [])
        types = types if isinstance(types, list) else [types]
        return '(c: "mtype", types: [%s], inverted: %s)' % (', '.join(q(x) for x in types), 'true' if c.get('inverted') else 'false')
    if t == 'MobStateCondition':
        return '(c: "mobstate", state: %s, inverted: %s)' % (q(c.get('mobstate', 'Alive')), 'true' if c.get('inverted') else 'false')
    if t == 'TemperatureCondition':
        return '(c: "temp", min: %s, max: %s)' % (f(c.get('min', 0)), f(c.get('max', -1)))
    return None


def types_of(spec):
    """DamageSpecifier → {вид: число}: группы делятся поровну между видами."""
    out = collections.OrderedDict()
    if not isinstance(spec, dict):
        return out
    for k, v in (spec.get('types') or {}).items():
        out[k] = out.get(k, 0) + float(v)
    for g, v in (spec.get('groups') or {}).items():
        ts = GROUPS.get(g, [])
        for t in ts:
            out[t] = out.get(t, 0) + float(v) / len(ts)
    return out


def effect(e):
    t = e.get('__tag')
    body = None
    if t == 'HealthChange':
        d = types_of(e.get('damage') or {})
        if not d:
            return None
        body = 'k: "health", damage: {%s}' % ', '.join('%s: %s' % (q(k), f(v)) for k, v in d.items())
    elif t in ('EvenHealthChange', 'DistributedHealthChange'):
        d = e.get('damage') or {}
        body = 'k: "%s", damage: {%s}' % ('even' if t == 'EvenHealthChange' else 'dist', ', '.join('%s: %s' % (q(k), f(v)) for k, v in d.items()))
    elif t == 'Satiate':
        body = 'k: "satiate", type: %s, factor: %s' % (q(e.get('satiationType', 'Hunger')), f(e.get('factor', 1)))
    elif t == 'ModifyBleed':
        body = 'k: "bleed", amount: %s' % f(e.get('amount', 0))
    elif t == 'ModifyBloodLevel':
        body = 'k: "blood", amount: %s' % f(e.get('amount', 0))
    elif t == 'Oxygenate':
        body = 'k: "oxygenate", factor: %s' % f(e.get('factor', 1))
    elif t == 'ModifyLungGas':
        body = 'k: "lunggas", ratios: {%s}' % ', '.join('%s: %s' % (q(k), f(v)) for k, v in (e.get('ratios') or {}).items())
    elif t == 'AdjustReagent':
        body = 'k: "adjust", reagent: %s, amount: %s' % (q(e.get('reagent', '')), f(e.get('amount', 0)))
    else:
        skipped[t] += 1
        return None
    cs = []
    for c in e.get('conditions') or []:
        s = cond(c)
        if s is None:
            skipped['условие ' + str(c.get('__tag'))] += 1
            return None
        cs.append(s)
    kept[t] += 1
    return '(%s, minScale: %s, prob: %s, conds: [%s])' % (body, f(e.get('minScale', 0)), f(e.get('probability', 1)), ', '.join(cs))


lines = [
    '// Реагенты SS14 (Resources/Prototypes/Reagents). Создано scripts/gen_chem.py.',
    '[',
]
n = 0
for rid, r in sorted(P.by_type['reagent'].items()):
    mets = []
    for stage, m in (r.get('metabolisms') or {}).items():
        if stage not in ('Bloodstream', 'Digestion', 'Respiration'):
            continue
        effs = [x for x in (effect(e) for e in (m.get('effects') or [])) if x]
        mets.append('%s: (rate: %s, effects: [%s])' % (q(stage), f(m.get('metabolismRate', 0.5)), ', '.join(effs)))
    name = names.get(r.get('name', ''), rid)
    lines.append('    (id: %s, components: {"Reagent": (name: %s, color: %s, group: %s, deadWorks: %s, metabolisms: {%s})}),' % (
        q('Reagent' + rid), q(name), q(r.get('color', '#ffffff')), q(r.get('group', '')),
        'true' if r.get('worksOnTheDead') else 'false', ', '.join(mets)))
    n += 1
lines.append(']')
open('content/prototypes/reagents.ron', 'w').write('\n'.join(lines) + '\n')
print(n, 'реагентов; эффектов перенесено', sum(kept.values()), kept.most_common(), '\nотброшено', sum(skipped.values()), skipped.most_common(12))

rl = ['// Реакции SS14 без смесителя при комнатной температуре. Создано scripts/gen_chem.py.', '[']
total = 0
dropped = collections.Counter()
for rid, r in sorted(P.by_type['reaction'].items()):
    total += 1
    if not r.get('reactants'):
        continue
    reac = ', '.join('%s: (amount: %s, catalyst: %s)' % (q(k), f(v.get('amount', 1)), 'true' if v.get('catalyst') else 'false') for k, v in r['reactants'].items())
    prod = ', '.join('%s: %s' % (q(k), f(v)) for k, v in (r.get('products') or {}).items())
    rl.append('    (id: %s, components: {"Reaction": (priority: %d, quantized: %s, reactants: {%s}, products: {%s}, minTemp: %s, maxTemp: %s, conserveEnergy: %s, mixers: [%s])}),' % (
        q('Reaction' + rid), int(r.get('priority', 0)), 'true' if r.get('quantized') else 'false', reac, prod,
        f(r.get('minTemp', 0)), f(r.get('maxTemp', 1e30)), 'false' if r.get('conserveEnergy') is False else 'true',
        ', '.join(q(m) for m in r.get('requiredMixerCategories') or [])))
rl.append(']')
open('content/prototypes/reactions.ron', 'w').write('\n'.join(rl) + '\n')
print(total - sum(dropped.values()), 'реакций из', total, dict(dropped))
