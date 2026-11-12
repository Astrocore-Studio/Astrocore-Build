#!/usr/bin/env python3
"""Химические машины SS14 → content/prototypes/chem_machines.ron (создано scripts/gen_chem_machines.py).

ХимМастер, плитка, настольные центрифуга и электролизёр: спрайт, потребитель тока и параметры
компонентов (`ChemMaster`, `SolutionHeater`, `SolutionContainerMixer`, `ReactionMixer`).
Использование: gen_chem_machines.py <корень SS14>."""
import json
import sys
from ss14yaml import Protos

P = Protos(sys.argv[1])
q = lambda s: json.dumps(s, ensure_ascii=False)
MACHINES = ['ChemMaster', 'ChemistryHotplate', 'MachineCentrifuge', 'MachineElectrolysisUnit', 'CryoPod']


def sprite(c):
    sp = c.get('Sprite', {})
    rsi, state = sp.get('sprite'), sp.get('state')
    for l in sp.get('layers') or []:
        # Слои чужих наборов (труба под капсулой) не годятся как основной спрайт.
        if state is None and isinstance(l, dict) and l.get('state') and 'sprite' not in l:
            state = l['state']
    return rsi.removeprefix('/Textures/').lstrip('/'), state


def num(x, d):
    return float(x) if x is not None else d


lines = ['// Химические машины SS14. Создано scripts/gen_chem_machines.py.', '[']
for p in MACHINES:
    c = P.entity_components(p)
    rsi, state = sprite(c)
    comps = ['"Sprite": (sprite: %s, state: %s)' % (q(rsi), q(state))]
    load = (c.get('ApcPowerReceiver') or {}).get('powerLoad')
    comps.append('"ApcPowerReceiver": (load: %s)' % num(load, 200.0))
    if 'ChemMaster' in c:
        comps.append('"ChemMaster": (pillDosageLimit: %d)' % int(c['ChemMaster'].get('pillDosageLimit', 20)))
    if 'SolutionHeater' in c:
        comps.append('"SolutionHeater": (heatPerSecond: %s)' % num(c['SolutionHeater'].get('heatPerSecond'), 0.0))
    if 'SolutionContainerMixer' in c:
        comps.append('"SolutionContainerMixer": (mixDuration: %s)' % num(c['SolutionContainerMixer'].get('mixDuration'), 10.0))
    if 'ReactionMixer' in c:
        comps.append('"ReactionMixer": (types: [%s], mixerType: "Machine", time: 0.0)' % ', '.join(q(t) for t in c['ReactionMixer'].get('reactionTypes') or []))
    if 'CryoPod' in c:
        cp = c['CryoPod']
        comps.append('"CryoPod": (beakerTransferTime: %s, beakerTransferAmount: %s, entryDelay: %s)' % (
            num(cp.get('beakerTransferTime'), 2.0), num(cp.get('beakerTransferAmount'), 1.0), num(cp.get('entryDelay'), 2.0)))
        comps.append('"GasPipe": (nodes: [(dirs: "S")])')
        comps.append('"CryoPodAir": (node: 0, volume: 2000.0)')
    lines.append('    (id: %s, name: Some(%s), components: {%s}),' % (q(p), q(P.entity_name(p) or p), ', '.join(comps)))
lines.append(']')
open('content/prototypes/chem_machines.ron', 'w').write('\n'.join(lines) + '\n')
print(len(MACHINES), 'машин')
