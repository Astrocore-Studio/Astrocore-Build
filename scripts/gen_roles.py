#!/usr/bin/env python3
"""Должности SS14 → content/prototypes/jobs.ron.

Для каждой должности из `availableJobs` карты: название, число мест, снаряжение. Снаряжение —
как в `StationSpawningSystem.SpawnPlayerMob`: сначала набор по умолчанию (`RoleLoadout.SetDefault`:
первые подходящие варианты каждой группы), потом `startingGear` должности поверх. Использование:
gen_roles.py <корень SS14> <имя карты в Maps>."""
import re
import sys
import json
from ss14yaml import Protos


def ron_str(s):
    return json.dumps(s, ensure_ascii=False)


def main():
    root, mapid = sys.argv[1], sys.argv[2]
    P = Protos(root)
    gm = P.get('gameMap', mapid)
    station = list(gm['stations'].values())[0]
    jobs_comp = next(c for c in station['components'] if c['type'] == 'StationJobs')
    avail = jobs_comp['availableJobs']

    def gear_of(gid):
        g = P.get('startingGear', gid)
        if not g:
            return {}, [], {}
        eq, ih, st = {}, [], {}
        for e in P.chain('startingGear', gid):
            eq.update(e.get('equipment') or {})
            ih.extend(e.get('inhand') or [])
            for k, v in (e.get('storage') or {}).items():
                st.setdefault(k, []).extend(v)
        return eq, ih, st

    def effect_ok(eff):
        # Профиль по умолчанию: вид Human, наигранного времени нет (RoleLoadout.IsValid).
        tag = eff.get('__tag')
        if tag == 'GroupLoadoutEffect':
            g = P.get('loadoutEffectGroup', eff['proto'])
            return all(effect_ok(e) for e in (g or {}).get('effects', []))
        if tag == 'SpeciesLoadoutEffect':
            return 'Human' in eff.get('species', [])
        if tag == 'JobRequirementLoadoutEffect':
            return bool(eff.get('requirement', {}).get('inverted'))
        return True

    def loadout_ok(lid):
        l = P.get('loadout', lid)
        return bool(l) and all(effect_ok(e) for e in l.get('effects', []))

    def loadout_gear(lid):
        l = P.get('loadout', lid)
        if not l:
            return {}, [], {}
        eq = dict(l.get('equipment') or {})
        ih = list(l.get('inhand') or [])
        st = {k: list(v) for k, v in (l.get('storage') or {}).items()}
        if l.get('startingGear'):
            e2, i2, s2 = gear_of(l['startingGear'])
            e2.update(eq)
            eq = e2
            ih += i2
            for k, v in s2.items():
                st.setdefault(k, []).extend(v)
        return eq, ih, st

    lines = [
        '// Должности (Resources/Prototypes/Roles/Jobs, Loadouts) для карты %s. Создано scripts/gen_roles.py.' % mapid,
        '// `slots` — (на старте, максимум), -1 — без ограничения; `equipment` — слот → прототип SS14;',
        '// набор по умолчанию (RoleLoadout.SetDefault) уже подмешан, поверх него лёг startingGear должности.',
        '[',
    ]
    names = {}
    ftl = open(root + '/Resources/Locale/en-US/job/job-names.ftl').read()
    for m in re.finditer(r'^(job-name-[\w-]+) = (.+)$', ftl, re.M):
        names[m.group(1)] = m.group(2)
    weights = P.get('jobWeight', 'Default')['weights']
    n = 0
    for jid, (start, mx) in avail.items():
        job = P.get('job', jid)
        if not job:
            continue
        eq, inhand, storage = {}, [], {}
        # Наборы по умолчанию: группы роли в порядке от последней к первой, как SetDefault.
        rl = P.get('roleLoadout', 'Job' + jid)
        if rl:
            picked = []
            for gid in reversed(rl.get('groups', [])):
                g = P.get('loadoutGroup', gid)
                if not g:
                    continue
                lim = max(g.get('minLimit', 1), g.get('defaultSelected', 0))
                cnt = 0
                for lid in g.get('loadouts', []):
                    if cnt >= lim:
                        break
                    if not loadout_ok(lid):
                        continue
                    picked.append((gid, lid))
                    cnt += 1
            # Экипируются в порядке групп роли.
            order = {g: i for i, g in enumerate(rl.get('groups', []))}
            for gid, lid in sorted(picked, key=lambda x: order[x[0]]):
                e, i, s = loadout_gear(lid)
                eq.update(e)
                inhand += i
                for k, v in s.items():
                    storage.setdefault(k, []).extend(v)
        if job.get('startingGear'):
            e, i, s = gear_of(job['startingGear'])
            eq.update(e)
            inhand += i
            for k, v in s.items():
                storage.setdefault(k, []).extend(v)
        title = names.get(job.get('name', ''), jid)
        access = list(job.get('access') or [])
        for g in job.get('accessGroups') or []:
            access += (P.get('accessGroup', g) or {}).get('tags', [])
        access = sorted(set(access))
        eqs = ', '.join('%s: %s' % (ron_str(k), ron_str(v)) for k, v in sorted(eq.items()))
        ihs = ', '.join(ron_str(x) for x in inhand)
        sts = ', '.join('%s: [%s]' % (ron_str(k), ', '.join(ron_str(x) for x in v)) for k, v in sorted(storage.items()))
        lines.append(
            '    (id: "Job%s", name: Some(%s), components: {"Job": (title: %s, start: %d, max: %d, weight: %d, access: [%s], equipment: {%s}, inhand: [%s], storage: {%s})}),'
            % (jid, ron_str(title), ron_str(title), start, mx, int(weights.get(jid, 0)), ', '.join(ron_str(a) for a in access), eqs, ihs, sts)
        )
        n += 1
    lines.append(']')
    open('content/prototypes/jobs.ron', 'w').write('\n'.join(lines) + '\n')
    print(n, 'должностей')


main()
