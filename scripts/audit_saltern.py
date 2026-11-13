#!/usr/bin/env python3
"""Inventory the local SS14 map and Build prototypes without changing either.

Usage: python scripts/audit_saltern.py <SS14> [--output .local/reports/saltern-audit.json]
"""
import argparse
import collections
import json
import re
from pathlib import Path

from ss14yaml import Loader, Protos
import yaml


def git_head(root):
    """Read the local revision without fetching or modifying a source checkout."""
    dotgit = root / '.git'
    if dotgit.is_file():
        dotgit = (root / dotgit.read_text(encoding='utf-8').strip().removeprefix('gitdir: ')).resolve()
    head = (dotgit / 'HEAD').read_text(encoding='utf-8').strip()
    if not head.startswith('ref: '):
        return head
    ref = head.removeprefix('ref: ')
    path = dotgit / ref
    if path.exists():
        return path.read_text(encoding='utf-8').strip()
    for line in (dotgit / 'packed-refs').read_text(encoding='utf-8').splitlines():
        if line.endswith(' ' + ref):
            return line.split()[0]
    raise ValueError(f'Cannot resolve {ref} in {root}')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('source', type=Path)
    parser.add_argument('--output', type=Path,
                        default=Path('.local/reports/saltern-audit.json'))
    args = parser.parse_args()
    build = Path(__file__).resolve().parent.parent
    source = args.source.resolve()
    map_path = source / 'Resources/Maps/saltern.yml'
    with map_path.open(encoding='utf-8') as stream:
        data = yaml.load(stream, Loader=Loader)
    counts = collections.Counter()
    for group in data['entities']:
        counts[group.get('proto', '')] += len(group.get('entities', []))
    actual_count = sum(counts.values())
    expected_count = data['meta']['entityCount']
    if actual_count != expected_count:
        raise ValueError(f'Map entity count differs: {actual_count} != {expected_count}')

    build_ids = set()
    computer_ids = set()
    for path in (build / 'content/prototypes').glob('*.ron'):
        ids = set(re.findall(r'\(id:\s*"([^\"]+)"', path.read_text(encoding='utf-8')))
        build_ids.update(ids)
        if path.name == 'computers.ron':
            computer_ids = ids

    print(f'Map: {actual_count} entities, {len(counts)} prototype groups. Reading SS14 prototypes...',
          flush=True)
    protos = Protos(source.as_posix())
    computers = []
    for ident in sorted(computer_ids):
        components = protos.entity_components(ident)
        computers.append({
            'prototype': ident,
            'count': counts[ident],
            'name': protos.entity_name(ident),
            'build_status': 'implemented; see docs/saltern-computers.md for verification'
                            if ident in ('ComputerId', 'ComputerPowerMonitoring', 'ComputerAlert')
                            else 'visual and power only',
            'ss14_components': sorted(components),
            'ss14_ui': components.get('UserInterface', {}).get('interfaces', {}),
        })
    result = {
        'source': str(source),
        'source_revision': git_head(source),
        'build_revision': git_head(build),
        'engine_revision': git_head(build.parent / 'Astrocore-Engine'),
        'map': str(map_path),
        'map_format': data['meta']['format'],
        'entities': actual_count,
        'prototype_groups': len(counts),
        'computers': computers,
        'prototypes': [{
            'prototype': ident,
            'count': count,
            'in_build_registry': ident in build_ids,
            'ss14_components': sorted(protos.entity_components(ident)) if ident else [],
        } for ident, count in sorted(counts.items())],
        'notes': [
            'Registry presence does not prove that a mechanic or its assets are implemented.',
            'Geometric objects and cable zones can be imported without a matching entity prototype.',
            'Computer statuses are maintained explicitly; verification details are in docs/saltern-computers.md.',
        ],
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
    print(f'Written {args.output}; {len(computers)} computer types, '
          f'{sum(row["count"] for row in computers)} computers on Saltern.')
    for row in computers:
        print(f'{row["prototype"]}: {row["count"]} — {row["build_status"]}')


if __name__ == '__main__':
    main()
