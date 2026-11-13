#!/usr/bin/env python3
"""Описания прототипов для осмотра (`ExamineSystem`: `MetaData.EntityDescription`).

Для каждого прототипа из content/prototypes/*.ron берётся последнее непустое `description` по цепочке
родителей SS14. Результат: content/descriptions.ron. Использование:
  python3 scripts/gen_descriptions.py <корень SS14>
"""
import glob
import re
import sys

sys.path.insert(0, 'scripts')
from ss14yaml import Protos


def q(s):
    return '"' + s.replace('\\', '\\\\').replace('"', '\\"').replace('\n', ' ') + '"'


def main():
    root = sys.argv[1]
    P = Protos(root)
    ids = set()
    for f in glob.glob('content/prototypes/*.ron'):
        ids |= set(re.findall(r'\(id: "([^"]+)"', open(f, encoding='utf-8').read()))
    out = ['// Описания прототипов для осмотра: id → текст SS14 (`description`). Генерируется scripts/gen_descriptions.py.', '[']
    for i in sorted(ids):
        d = [e.get('description') for e in P.chain('entity', i) if isinstance(e.get('description'), str) and e.get('description').strip()]
        if d:
            out.append('    (%s, %s),' % (q(i), q(d[-1].strip())))
    out.append(']')
    open('content/descriptions.ron', 'w', encoding='utf-8').write('\n'.join(out) + '\n')
    print('описаний:', len(out) - 3)


if __name__ == '__main__':
    main()
