#!/usr/bin/env python3
"""Export ComputerAlert/FireAlarm data and RSI frames from the local SS14 source.

Run from Build: python scripts/gen_atmos_alerts.py D:/projects/space-station-14
Composites retain the source layers, direction frames and animation delays.
"""
import argparse
import json
import math
import shutil
import hashlib
from fractions import Fraction
from pathlib import Path

import yaml
from PIL import Image
from audit_saltern import git_head
from ss14yaml import Protos, Loader


def ron(value):
    if isinstance(value, str):
        return json.dumps(value, ensure_ascii=False)
    if isinstance(value, bool):
        return str(value).lower()
    if isinstance(value, dict):
        return '(' + ', '.join(f'{key}: {ron(item)}' for key, item in value.items()) + ')'
    if isinstance(value, list):
        return '[' + ', '.join(ron(item) for item in value) + ']'
    return str(value)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('source', type=Path)
    args = parser.parse_args()
    source = args.source.resolve()
    build = Path(__file__).resolve().parent.parent
    revision = git_head(source)
    stamp = hashlib.sha256('\n'.join(f'{p}:{p.stat().st_size}:{p.stat().st_mtime_ns}'
        for p in sorted((source / 'Resources/Prototypes').rglob('*.yml'))).encode()).hexdigest()
    textures = source / 'Resources/Textures'
    assets = build / 'assets/ss14'
    roles = {'ComputerAlert': 'Console', 'AirAlarm': 'AirAlarm', 'FireAlarm': 'FireAlarm',
             'AirSensor': 'Sensor', 'GasVentPump': 'Sensor', 'GasVentScrubber': 'Sensor'}
    cache_path = build / '.local/reports/atmos-alerts-definitions.json'
    cached = json.loads(cache_path.read_text(encoding='utf-8')) if cache_path.exists() else {}
    if cached.get('revision') == revision and cached.get('stamp') == stamp and set(cached.get('components', {})) == set(roles):
        components, threshold_protos, names = cached['components'], cached['thresholds'], cached['names']
    else:
        protos = Protos(source.as_posix())
        # unary.yml uses CreateVariants IDs, which the older shared loader skips.
        # Resolve its zipped fields exactly by variant index before inheritance.
        def variant(value, index):
            if isinstance(value, dict):
                if value.get('__tag') == 'CreateVariants':
                    return variant((value.get('values') or value['sequences'])[index], index)
                return {key: variant(item, index) for key, item in value.items()}
            if isinstance(value, list):
                return [variant(item, index) for item in value]
            return value
        unary = source / 'Resources/Prototypes/Entities/Structures/Piping/Atmospherics/unary.yml'
        for entity in yaml.load(unary.read_text(encoding='utf-8'), Loader=Loader):
            ident = entity.get('id')
            if isinstance(ident, dict) and ident.get('__tag') == 'CreateVariants':
                for index, name in enumerate(ident['values']):
                    protos.by_type['entity'][name] = variant(entity, index)
        components = {name: protos.entity_components(name) for name in roles}
        threshold_protos = protos.by_type['alarmThreshold']
        names = {name: protos.entity_name(name) or name for name in roles}
        cache_path.parent.mkdir(parents=True, exist_ok=True)
        cache_path.write_text(json.dumps({'revision': revision, 'stamp': stamp, 'components': components,
            'thresholds': threshold_protos, 'names': names}), encoding='utf-8')
    if any(not value for value in components.values()):
            raise ValueError('Required SS14 prototype not found')
    data, visual_data = [], {}
    for name, role in roles.items():
        comp = components[name]
        monitor = comp.get('AtmosMonitor', {})
        alarmable = comp.get('AtmosAlarmable', {})
        thresholds = {}
        for field, key in [('temperatureThresholdId', 'temperature'), ('pressureThresholdId', 'pressure')]:
            ident = monitor.get(field)
            if ident:
                thresholds[key] = {k: v['threshold'] for k, v in threshold_protos[ident].items()
                                   if k in ('upperBound', 'lowerBound', 'upperWarnAround', 'lowerWarnAround') and v.get('enabled', True)}
        gases = []
        for gas, ident in monitor.get('gasThresholdPrototypes', {}).items():
            threshold = threshold_protos[ident]
            gases.append({'gas': gas, **{k: v['threshold'] for k, v in threshold.items()
                                        if k in ('upperBound', 'lowerBound', 'upperWarnAround', 'lowerWarnAround') and v.get('enabled', True)}})
        data.append({'prototype': name, 'role': role, 'name': names[name],
                     'tags': comp.get('Tag', {}).get('tags', []),
                     'syncWith': alarmable.get('syncWith', []),
                     'monitorAlertTypes': alarmable.get('monitorAlertTypes', []),
                     'receiveOnly': alarmable.get('receiveOnly', False),
                     'monitorFire': monitor.get('monitorFire', False),
                     'visual': name in ('ComputerAlert', 'AirAlarm', 'FireAlarm', 'AirSensor'),
                     'wallMount': 'WallMount' in comp,
                     **thresholds, 'gases': gases})
        if name not in ('ComputerAlert', 'AirAlarm', 'FireAlarm', 'AirSensor'):
            continue
        sprite = comp['Sprite']['sprite']
        rsi = textures / sprite
        meta = json.loads((rsi / 'meta.json').read_text(encoding='utf-8'))
        license_name = meta['license']
        if '-NC' in license_name or '-ND' in license_name or not license_name.startswith(('CC-BY', 'CC0')):
            raise ValueError(f'{sprite}: forbidden license {license_name}')
        size = (meta['size']['x'], meta['size']['y'])
        states = {state['name']: state for state in meta['states']}
        if role == 'Console':
            computer = comp['ComputerVisuals']
            visualizer = next(iter(comp['GenericVisualizer']['visuals'].values()))
            mapping = next(iter(visualizer.values()))
            frames = [computer.get('stateFrame'), computer.get('stateKeyboard')]
            variants = {'Invalid': frames + [computer.get('stateKeys')]}
            for level, number in [('Normal', 1), ('Warning', 2), ('Danger', 3)]:
                variants[level] = frames + [(mapping.get(number) or mapping[str(number)])['state'], computer.get('stateKeys')]
        elif role == 'Sensor':
            variants = {'Invalid': ['gsensor0'], 'Normal': ['gsensor1']}
        else:
            visualizer = comp['AtmosAlarmableVisuals']
            # Maintenance panel is hidden until opened in SS14. Keep the base and alarm layers.
            variants = {'Invalid': ['base']}
            variants.update({level: ['base', visualizer['alarmStates'][level]]
                             for level in ('Normal', 'Warning', 'Danger')})
        output = assets / 'Textures/Structures/AtmosAlerts_generated' / f'{name}.rsi'
        output.mkdir(parents=True, exist_ok=True)
        exported_states, animations = [], {}
        for level, layer_names in variants.items():
            direction_count = max(states[n].get('directions', 1) for n in filter(None, layer_names))
            direction_animations = []
            for direction in range(direction_count):
                layers = []
                for state_name in filter(None, layer_names):
                    state = states[state_name]
                    direction_index = direction if state.get('directions', 1) > 1 else 0
                    delays = [Fraction(str(v)) for v in state.get('delays', [[1]] * direction_count)[direction_index]]
                    image = Image.open(rsi / f'{state_name}.png').convert('RGBA')
                    images = []
                    columns = image.width // size[0]
                    for frame_index in range(len(delays)):
                        index = direction_index * len(delays) + frame_index
                        x, y = index % columns * size[0], index // columns * size[1]
                        images.append(image.crop((x, y, x + size[0], y + size[1])))
                    layers.append((delays, images))
                animated = [sum(delays) for delays, images in layers if len(images) > 1]
                denominator = math.lcm(*(period.denominator for period in animated)) if animated else 1
                period = Fraction(math.lcm(*(int(p * denominator) for p in animated)), denominator) if animated else Fraction(1)
                boundaries = {Fraction(0), period}
                for delays, images in layers:
                    if len(images) == 1:
                        continue
                    cursor = Fraction(0)
                    while cursor < period:
                        for delay in delays:
                            cursor += delay
                            if cursor < period:
                                boundaries.add(cursor)
                boundaries = sorted(boundaries)
                images, delays_out = [], []
                for start, end in zip(boundaries, boundaries[1:]):
                    composite = Image.new('RGBA', size)
                    for delays, layer in layers:
                        at, index = start % sum(delays), 0
                        while index + 1 < len(delays) and at >= delays[index]:
                            at -= delays[index]
                            index += 1
                        composite.alpha_composite(layer[index])
                    filename = f'{level.lower()}_{direction}_{len(images)}'
                    composite.save(output / f'{filename}.png')
                    images.append((output / f'{filename}.png').relative_to(assets).as_posix())
                    delays_out.append(float(end - start))
                    exported_states.append({'name': filename})
                direction_animations.append({'images': images, 'delays': delays_out})
            animations[level] = {'directions': direction_animations}
        (output / 'meta.json').write_text(json.dumps({
            'version': 1, 'license': license_name, 'copyright': meta.get('copyright', ''),
            'source': f'SS14 {revision}, Resources/Textures/{sprite}; generated by gen_atmos_alerts.py',
            'size': meta['size'], 'states': exported_states}, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
        visual_data[name] = animations
    # FireAlarm was absent from Build. Export its data instead of editing existing generated files.
    fire = components['FireAlarm']
    # ApcPowerReceiverComponent.NetworkLoad.DesiredPower defaults to 5 W.
    load = float(fire.get('ApcPowerReceiver', {}).get('powerLoad', 5))
    sprite = 'Structures/AtmosAlerts_generated/FireAlarm.rsi'
    output = [f'// Generated by scripts/gen_atmos_alerts.py. SS14: {revision}', '[',
              '    (id: "FireAlarm", name: Some(' + ron(names['FireAlarm']) + '), components: {',
              '        "Sprite": (sprite: ' + ron(sprite) + ', state: "normal_0_0"),',
              '        "ApcPowerReceiver": (load: ' + str(load) + '),',
              '    }),',
              '    (id: "AtmosAlertsData", abstract: true, components: {',
              '        "AtmosAlerts": (devices: ' + ron(data) + '),', '    }),', ']']
    (build / 'content/prototypes/atmos_alerts.ron').write_text('\n'.join(output) + '\n', encoding='utf-8')
    alarms = [f'// Generated by scripts/gen_atmos_alerts.py. SS14: {revision}', '[']
    for name in ('AirAlarm', 'AirSensor'):
        load = float(components[name].get('ApcPowerReceiver', {}).get('powerLoad', 5))
        alarms += ['    (id: ' + ron(name) + ', name: Some(' + ron(names[name]) + '), components: {',
                   '        "Sprite": (sprite: ' + ron(f'Structures/AtmosAlerts_generated/{name}.rsi') + ', state: "normal_0_0"),',
                   '        "ApcPowerReceiver": (load: ' + str(load) + '),']
        if 'AtmosMonitor' in components[name]:
            alarms.append('        "AtmosMonitor": (kind: "sensor"),')
        alarms.append('    }),')
    alarms.append(']')
    (build / 'content/prototypes/alarms.ron').write_text('\n'.join(alarms) + '\n', encoding='utf-8')
    (assets / 'atmos-alerts-visuals.json').write_text(json.dumps({
        'source_revision': revision, 'devices': visual_data}, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
    audio_root = source / 'Resources/Audio/Machines'
    attribution = next((row for row in yaml.safe_load((audio_root / 'attributions.yml').read_text(encoding='utf-8'))
                        if 'alarm.ogg' in row.get('files', [])),
                       {'license': 'CC-BY-SA-3.0', 'copyright':
                        'No per-file entry in Machines/attributions.yml; default asset license from the SS14 README.'})
    lic = attribution['license']
    if '-NC' in lic or '-ND' in lic or not lic.startswith(('CC0', 'CC-BY')):
        raise ValueError(f'alarm.ogg: forbidden license {lic}')
    destination = assets / 'Audio/Machines/alarm.ogg'
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(audio_root / 'alarm.ogg', destination)
    (assets / 'Audio/ATMOS_ALERTS_ATTRIBUTIONS.md').write_text(
        f'# Atmos alarm sound\n\n`Machines/alarm.ogg`: SS14 `{revision}`, copied unchanged.\n\n'
        f'License: {lic}\n\n{attribution.get("copyright", "")}\n', encoding='utf-8')
    # Preserve exactly the map's device names and addresses through the importer.
    with (source / 'Resources/Maps/saltern.yml').open(encoding='utf-8') as stream:
        map_data = yaml.load(stream, Loader=Loader)
    report = []
    for group in map_data['entities']:
        if group.get('proto') in roles:
            report.append(group)
    report_path = build / '.local/reports/atmos-alerts-source.json'
    report_path.parent.mkdir(parents=True, exist_ok=True)
    report_path.write_text(json.dumps({'revision': revision, 'devices': data, 'entities': report}, indent=2) + '\n', encoding='utf-8')
    print(f'Exported {len(data)} device types, {len(visual_data)} visual sets and alarm.ogg from {revision}')


if __name__ == '__main__':
    main()
