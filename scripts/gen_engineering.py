"""Export the engineering graphs with original typed steps, conditions and callbacks."""
import argparse
import copy
import json
import re
import shutil
from pathlib import Path

import yaml

from ss14yaml import Loader, Protos

FAMILIES = {"structures", "utilities", "machines", "materials", "furniture", "atmospherics"}


def variant(value, i):
    if isinstance(value, dict):
        if value.get("__tag") == "CreateVariants":
            return variant((value.get("values") or value["sequences"])[i], i)
        return {key: variant(item, i) for key, item in value.items()}
    if isinstance(value, list):
        return [variant(item, i) for item in value]
    return value


def expand(entries):
    for entry in entries:
        ids = entry.get("id")
        if isinstance(ids, dict):
            for index in range(len(ids["values"])):
                yield variant(entry, index)
        else:
            yield entry


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    args = parser.parse_args()
    root = args.source / "Resources/Prototypes/Recipes/Construction"
    graphs = {}
    recipes = {}
    for file in sorted(root.rglob("*.yml")):
        path = file.relative_to(root)
        if path.parts[0] == "Graphs" and path.parts[1] not in FAMILIES:
            continue
        for entry in expand(yaml.load(file.read_text(encoding="utf-8"), Loader=Loader) or []):
            if entry["type"] == "constructionGraph":
                graphs[entry["id"]] = dict(entry, source=path.as_posix())
            elif entry["type"] == "construction":
                recipes[entry["id"]] = dict(entry, source=path.as_posix())

    def resolve(name, ancestors=()):
        if name in ancestors:
            raise ValueError(f"Cyclic construction recipe: {name}")
        local = recipes[name]
        parents = local.get("parent", [])
        result = {}
        for parent in [parents] if isinstance(parents, str) else parents:
            result.update(resolve(parent, (*ancestors, name)))
        result.pop("abstract", None)
        result.update(local)
        return result

    entries = [resolve(name) for name in sorted(recipes)]
    entries = [e for e in entries if not e.get("abstract") and e.get("graph") in graphs]
    materials = {step["material"] for graph in graphs.values() for node in graph["graph"]
                 for edge in node.get("edges", []) for step in edge.get("steps", [])
                 if "material" in step}
    source = Protos(str(args.source))
    audio_root = args.source / "Resources/Audio"
    audio_rows = {}
    audio_skipped = {}

    def sound(spec):
        if isinstance(spec, str):
            spec = {"path": spec}
        if not spec:
            return None
        paths = ([spec["path"]] if spec.get("path") else
                 source.merged_proto("soundCollection", spec.get("collection")).get("files", []))
        files = []
        for path in paths:
            rel = path.removeprefix("/Audio/")
            file = audio_root / rel
            if not file.is_file():
                audio_skipped[rel] = "Source file is not present locally"
                continue
            attr_file = file.with_name("attributions.yml")
            entries = yaml.safe_load(attr_file.read_text(encoding="utf8")) if attr_file.exists() else []
            attr = next((a for a in entries or [] if file.name in a.get("files", [])), {})
            license = attr.get("license", "CC-BY-SA-3.0")
            # The owner approved all SS14 asset licenses on 2026-11-15 (docs/HANDOFF.md).
            audio_rows[rel] = (license, attr.get("copyright", "Space Station 14 Resources"), attr.get("source", ""))
            files.append(rel)
        if files:
            return {"files": files, "volume": spec.get("params", {}).get("volume", 0)}
    pipe_file = args.source / "Resources/Prototypes/Entities/Structures/Piping/Atmospherics/pipes.yml"
    for filename in ("pipes.yml", "binary.yml", "trinary.yml", "unary.yml"):
        for proto in expand(yaml.load((pipe_file.parent / filename).read_text(encoding="utf8"), Loader=Loader)):
            source.by_type['entity'][proto['id']] = proto
    pipes = {}
    for entry in expand(yaml.load(pipe_file.read_text(encoding="utf8"), Loader=Loader)):
        components = {c["type"]: c for c in entry.get("components", [])}
        if components.get("Construction", {}).get("graph") not in ("GasPipe", "GasPipeAlt1", "GasPipeAlt2"):
            continue
        port = components.get("NodeContainer", {}).get("nodes", {}).get("pipe", {})
        if port.get("pipeDirection") in {"South", "Longitudinal", "SWBend", "TSouth", "Fourway"}:
            pipes[entry["id"]] = {"direction": port["pipeDirection"], "volume": port.get("volume", 200),
                                  "layer": components["AtmosPipeLayers"]["pipeLayer"]}
    stacks = {}
    build = Path(__file__).resolve().parents[1]
    tile_names = re.findall(r'tile: "(\w+)"', (build / "content/tiles.ron").read_text(encoding="utf8"))
    tiles = {}
    for name in tile_names:
        tile = source.merged_proto("tile", name)
        tiles[name.lower()] = {"name": name, "base": tile.get("baseTurf"),
                               "indestructible": tile.get("indestructible", False),
                               "subfloor": tile.get("isSubfloor", False), "sturdy": tile.get("sturdy", True),
                               "whitelist": tile.get("baseWhitelist", []),
                               "tools": tile.get("deconstructTools", []),
                               "drop": tile.get("itemDrop", "FloorTileItemSteel")}
    placed_by = {}
    tile_tools = {}
    welders = {}
    repairables, damageables, tool_speeds = {}, {}, {}
    cables = {}
    cable_objects = {}
    insulation = {}
    computer_boards = {}
    computers = {}
    machine_boards = {}
    tools_audio = {}
    machines = {}
    barriers = {}
    rcd_devices, rcd_ammo, rcd_targets = {}, {}, {}
    rcd_modes = {key:source.merged_proto("rcd",key) for key in source.by_type["rcd"]}
    inflatables, inflatable_items, balloon_poppers = {}, {}, []
    station_doors = set(re.findall(r"^- proto: (\w+)", (args.source / "Resources/Maps/saltern.yml").read_text(encoding="utf8"), re.M))
    airlocks, airlock_sprites, door_boards = {}, {}, {}
    airlock_settings, electrified, prying = {}, {}, {}
    board_configurators = []
    access_levels = sorted(name for name in source.by_type['accessLevel']
                           if source.merged_proto('accessLevel',name).get('name'))
    barrier_names = {"WallSolid", "WallReinforced", "Grille", "GrilleBroken", "InflatableWall"}
    barrier_names.update(n["entity"] for n in graphs["Window"]["graph"] if isinstance(n.get("entity"), str))
    girder_walls = {"WallUranium", "WallSilver", "WallBrass", "WallPlasma", "WallGold", "WallMeat", "WallShuttle"}
    barrier_names.update(girder_walls)
    part_traits = {}
    frame_ids = {"ComputerFrame", "ComputerBroken", "UnfinishedMachineFrame", "MachineFrame", "MachineFrameDestroyed", "AirlockAssembly", "Catwalk"}
    items = []
    prototypes = build / "content/prototypes"
    dest = prototypes / "engineering_tiles.ron"
    existing = set(re.findall(r'\(id:\s*"(\w+)"', '\n'.join(
        p.read_text(encoding="utf8") for p in prototypes.glob("*.ron") if p != dest)))
    # Only expose devices whose operating systems are already present in Build.
    atmos_defs = (prototypes / "atmos.ron").read_text(encoding="utf8")
    supported_devices = set(re.findall(r'\(id: "(\w+)", parents: \["GasDevice"\]', atmos_defs))
    for file in ("binary.yml", "trinary.yml", "unary.yml"):
        for entry in expand(yaml.load((pipe_file.parent / file).read_text(encoding="utf8"), Loader=Loader)):
            name = entry["id"]
            base = re.sub(r"Alt[12]$", "", name)
            if base not in supported_devices:
                continue
            comps = source.entity_components(name)
            if "Construction" not in comps:
                continue
            sprite = comps.get("Sprite", {})
            rsi = sprite.get("sprite")
            state = next((s["state"] for s in sprite.get("layers", []) if "state" in s and "sprite" not in s), sprite.get("state"))
            if not rsi or not state:
                continue
            src = args.source / "Resources/Textures" / rsi
            if not (src / (state + ".png")).is_file():
                continue
            dst = build / "assets/ss14/Textures" / rsi
            if not dst.exists():
                shutil.copytree(src, dst)
            layer = comps.get("AtmosPipeLayers", {}).get("pipeLayer", 0)
            nodes = comps['NodeContainer']['nodes']
            ordered = [key for key in ('inlet', 'filter', 'outlet') if key in nodes] or list(nodes)
            directions = {'North': 'N', 'South': 'S', 'East': 'E', 'West': 'W'}
            ports = [{'dirs': directions[nodes[key]['pipeDirection']], 'volume': nodes[key].get('volume', 200)} for key in ordered]
            pipes[name] = {"ports": ''.join(node['dirs'] for node in ports), "nodes": ports,
                           "layer": layer, "device": True, "sprite": rsi, "state": state}
            if name != base and name not in existing:
                items.append('    (id: %s, parents: [%s], components: {"Sprite": (sprite: %s, state: %s)}),' %
                             tuple(map(json.dumps, (name, base, rsi, state))))
    for name in sorted(source.by_type["entity"]):
        if source.is_abstract(name):
            continue
        components = source.entity_components(name)
        if 'Tool' in components:
            tool_speeds[name] = components['Tool'].get('speedModifier', 1)
        if 'Prying' in components:
            prying[name] = {k:v for k,v in components['Prying'].items() if k!='type'}
            prying[name]['useSound'] = sound(components['Prying'].get('useSound', '/Audio/Items/crowbar.ogg'))
        if 'Electrified' in components:
            electrified[name] = {k:v for k,v in components['Electrified'].items() if k!='type'}
            electrified[name]['shockNoises'] = sound(components['Electrified'].get('shockNoises', {'collection':'sparks'}))
        if 'DoorElectronicsConfigurator' in components.get('Tag', {}).get('tags', []):
            board_configurators.append(name)
        if "DoorElectronics" in components:
            door_boards[name] = components.get("AccessReader", {}).get("access", [])
            if name not in existing:
                items.append(f'    (id: "{name}", parents: ["DoorElectronics"]),')
        construction = components.get("Construction", {})
        if (name in station_doors or name in ("Airlock", "AirlockGlass")) and construction.get("graph") == "Airlock" and construction.get("node") in ("airlock", "glassAirlock"):
            sprite = components["Sprite"]["sprite"]
            src = args.source / "Resources/Textures" / sprite
            if all((src / (state + ".png")).is_file() for state in ("open", "closed", "opening", "closing")):
                material = "ss14:" + re.sub(r"(?<=[a-z0-9])(?=[A-Z])", "_", name).lower()
                boards = components.get("ContainerFill", {}).get("containers", {}).get("board", ["DoorElectronics"])
                airlocks[name] = {"material": material, "node": construction["node"], "board": boards[0], "wires": components.get("Wires", {}).get("layoutId", "Airlock")}
                airlock_settings[name] = {k:{field:value for field,value in components.get(k,{}).items() if field!='type'}
                                          for k in ('Airlock','Door','DoorBolt','ApcPowerReceiver')}
                airlock_settings[name]['tags'] = components.get('Tag',{}).get('tags',[])
                for field in ('openSound','closeSound'):
                    airlock_settings[name]['Door'][field] = sound(components.get('Door',{}).get(field))
                airlock_sprites[material] = sprite
                dst = build / "assets/ss14/Textures" / sprite
                if not dst.exists():
                    shutil.copytree(src, dst)
        if "RCD" in components:
            rcd = components["RCD"]
            charges = components.get("LimitedCharges", {})
            maximum = charges.get("maxCharges", 1)
            rcd_devices[name] = {"modes": rcd.get("availablePrototypes", []), "max": maximum,
                                 "initial": max(0, charges.get("lastCharges", maximum)), "sound":sound(rcd.get("successSound", "/Audio/Items/deconstruct.ogg")),
                                 "recharge":components.get("AutoRecharge", {}).get("rechargeDuration")}
        if "RCDAmmo" in components:
            rcd_ammo[name] = components["RCDAmmo"].get("charges",30)
        if "RCDDeconstructable" in components:
            dec = components["RCDDeconstructable"]
            if dec.get("deconstructable", True):
                rcd_targets[name] = {"cost":dec.get("cost",1), "delay":dec.get("delay",1)}
        if "InflatableSafeDisassembly" in components:
            disassembly = components["DisassembleOnAltVerb"]
            door = components.get("Door", {})
            inflatables[name] = {"returns": disassembly["prototypeToSpawn"], "duration": disassembly["disassembleTime"],
                                 "open": sound(door.get("openSound")), "close": sound(door.get("closeSound")),
                                 "pop": sound("/Audio/Effects/balloon-pop.ogg")}
        spawn = components.get("SpawnAfterInteract", {})
        if spawn.get("prototype") in ("InflatableWall", "InflatableDoor"):
            inflatable_items[name] = {"result": spawn["prototype"], "duration": spawn["doAfter"]}
        if "BalloonPopper" in components:
            balloon_poppers.append(name)
        if name in barrier_names:
            thresholds = [t.get("trigger", {}).get("damage") for t in components["Destructible"]["thresholds"]]
            resistance = source.merged_proto("damageModifierSet", components["Damageable"].get("damageModifierSet"))
            icon = components["Sprite"] if name == "InflatableWall" else components["Icon"]
            if name in girder_walls:
                src = args.source / "Resources/Textures" / icon["sprite"]
                dst = build / "assets/ss14/Textures" / icon["sprite"]
                if not dst.exists():
                    shutil.copytree(src, dst)
            barriers[name] = {"hp": min(t for t in thresholds if isinstance(t, (float, int))),
                              "coefficients": resistance.get("coefficients", {}), "flat": resistance.get("flatReductions", {}),
                              "rsi": icon["sprite"], "state": icon["state"]}
        if name in existing and "Tool" in components:
            sounds = {"use": sound(components["Tool"].get("useSound"))}
            if "Welder" in components:
                toggle = components.get("ItemToggle", {})
                sounds.update(on=sound(toggle.get("soundActivate")), off=sound(toggle.get("soundDeactivate")))
            tools_audio[name] = {k:v for k,v in sounds.items() if v}
        board = components.get("ComputerBoard", {}).get("prototype")
        if board and board in existing:
            computer_boards[name] = board
        installed = components.get("Computer", {}).get("board")
        if installed and name in existing:
            computers[name] = installed
        machine = components.get("MachineBoard", {})
        default_board = components.get("Machine", {}).get("board")
        if default_board and name in existing:
            spec = {"board": default_board}
            if "WiresPanel" in components:
                panel = components["WiresPanel"]
                spec["panel"] = {"tool": panel.get("openingTool", "Screwing"),
                                 "delay": panel.get("openDelay", 1), "open": panel.get("open", False),
                                 "openSound": sound(panel.get("screwdriverOpenSound", "/Audio/Machines/screwdriveropen.ogg")),
                                 "closeSound": sound(panel.get("screwdriverCloseSound", "/Audio/Machines/screwdriverclose.ogg"))}
            if "Lock" in components:
                spec["locked"] = components["Lock"].get("locked", True)
            machines[name] = spec
        if machine.get("prototype") in existing:
            machine_boards[name] = {k:v for k,v in machine.items() if k!="type"}
        if "Item" in components:
            part_traits[name] = (list(components), components.get("Tag", {}).get("tags",[]))
        if name in frame_ids and name not in existing:
            sprite = components["Sprite"]
            state = str(sprite.get("state", sprite.get("layers", [{}])[0].get("state")))
            if name=="Catwalk":
                state=components["Icon"]["state"]
            items.append('    (id: %s, name: Some(%s), components: {"Sprite": (sprite: %s, state: %s)}),' %
                         tuple(map(json.dumps,[name,source.entity_name(name),sprite["sprite"],state])))
        placer = components.get("CablePlacer", {})
        wire = placer.get("cablePrototypeID")
        group = {"HighVoltage": "hv", "MediumVoltage": "mv", "Apc": "apc"}.get(placer.get("blockingWireType","HighVoltage"))
        if wire and group:
            cables[name] = {"entity":wire, "group":group, "overTile":placer.get("overTile",False)}
        cable = components.get("Cable")
        if cable is not None:
            cable_objects[name] = {"tool": cable.get("cuttingQuality", "Cutting"),
                                   "delay": cable.get("cuttingDelay", 1),
                                   "drop": cable.get("cableDroppedOnCutPrototype", "CableHVStack1")}
        if "Insulated" in components:
            insulation[name] = components["Insulated"].get("coefficient", 0)
        welder = components.get("Welder")
        if welder is not None and "Welding" in components.get("Tool", {}).get("qualities", []):
            solution = components.get("Solution", {}).get("solution", {})
            volume = solution.get("maxVol")
            if volume:
                fuel = welder.get("fuelReagent", "WeldingFuel")
                welders[name] = {
                    "capacity": float(volume),
                    "initial": sum(float(r["Quantity"]) for r in solution.get("reagents", []) if r["ReagentId"] == fuel),
                    "fuel": fuel,
                    "litCost": float(welder.get("fuelLitCost", 0.5)),
                    "burnRate": float(welder.get("fuelConsumption", 1)),
                    "temperature": components.get("IgnitionSource", {}).get("temperature", 700),
                    "speed": components.get("Tool", {}).get("speedModifier", 1),
                    "refillable": "RefillableSolution" in components,
                }
        repair = components.get("Repairable")
        if repair is not None:
            repairables[name] = {k: v for k, v in repair.items() if k != "type"}
            if 'Damageable' in components:
                # Destructible.Thresholds uses AlwaysPushInheritance and merges equal triggers.
                thresholds = {}
                for ancestor in source.chain('entity', name):
                    for component in ancestor.get('components', []):
                        if component['type'] != 'Destructible':
                            continue
                        for threshold in component.get('thresholds', []):
                            key = json.dumps(threshold['trigger'], sort_keys=True)
                            current = thresholds.setdefault(key, {**copy.deepcopy(threshold), 'behaviors': []})
                            for behavior in threshold.get('behaviors', []):
                                if behavior not in current['behaviors']:
                                    current['behaviors'].append(copy.deepcopy(behavior))
                ordered = sorted(thresholds.values(), key=lambda t: t['trigger'].get('damage', 0))
                for threshold in ordered:
                    for behavior in threshold['behaviors']:
                        if behavior.get('__tag') == 'PlaySoundBehavior' and name in existing:
                            behavior['sourceSound'] = copy.deepcopy(behavior.get('sound'))
                            behavior['sound'] = sound(behavior.get('sound'))
                modifier = source.merged_proto('damageModifierSet', components['Damageable'].get('damageModifierSet'))
                damageables[name] = {'initial': components['Damageable'].get('damage', {}).get('types', {}),
                                    'coefficients': modifier.get('coefficients', {}),
                                    'flat': modifier.get('flatReductions', {}), 'thresholds': ordered,
                                    'vending': 'VendingMachine' in components}
        compatible = components.get("ToolTileCompatible")
        if compatible is not None:
            tile_tools[name] = float(compatible.get("delay", 1)) / float(components.get("Tool", {}).get("speedModifier", 1))
        stack = components.get("Stack", {})
        kind = stack.get("stackType")
        outputs = [t for t in components.get("FloorTile", {}).get("outputs", []) if t in tile_names]
        if outputs:
            placed_by[name] = outputs
        if (outputs or name in cables or name in computer_boards or name in machine_boards or
                name in rcd_devices or name in inflatable_items or name == "SheetGlass1" or
                kind in ("Manipulator", "Capacitor") or (kind in materials and stack.get("count") == 1)):
            if name not in existing:
                sprite = components.get("Sprite", {})
                rsi, state = sprite.get("sprite", ""), sprite.get("state", "")
                if rsi and state and (build / "assets/ss14/Textures" / rsi / (state+".png")).is_file():
                    q = json.dumps
                    items.append(f'    (id: {q(name)}, name: Some({q(source.entity_name(name) or name)}), components: {{"Item": (size: {q(components["Item"].get("size", "Normal"))}), "Sprite": (sprite: {q(rsi)}, state: {q(state)})}}),')
        if not kind or (kind not in materials and not outputs and name not in cables and name not in inflatable_items and kind not in ("Manipulator","Capacitor")):
            continue
        definition = source.merged_proto("stack", kind)
        stacks[name] = {"kind": kind, "initial": stack.get("count", 30),
                        "capacity": stack.get("maxCountOverride", definition.get("maxCount")),
                        "spawn": definition["spawn"]}
    target = Path(__file__).resolve().parents[1] / "content/engineering-graphs.json"
    component_names = {k for b in machine_boards.values() for k in b.get("componentRequirements",{})}
    tag_names = {k for b in machine_boards.values() for k in b.get("tagRequirements",{})}
    parts = {}
    for name,(comps,tags) in part_traits.items():
        traits = sorted(["component/"+k for k in comps if k in component_names] + ["tag/"+k for k in tags if k in tag_names])
        if traits: parts[name] = traits
    dest.write_text("// Generated by scripts/gen_engineering.py\n[\n" + "\n".join(items) + "\n]\n", encoding="utf8")
    # One entry per variant preserves the exact station prototype even where the RSI is shared.
    (build / "content/engineering-airlocks.json").write_text(json.dumps(airlocks,indent=2)+"\n", encoding="utf8")
    doors = build / "content/doors.ron"
    door_source = doors.read_text(encoding="utf8").split("    // Saltern variants")[0].rstrip().removesuffix("]").rstrip()
    variants = [f'    (id: "{key}", rsi: "{sprite}"),' for key,sprite in sorted(airlock_sprites.items()) if key not in ("ss14:airlock","ss14:airlock_glass")]
    doors.write_text(door_source+"\n    // Saltern variants from scripts/gen_engineering.py\n"+"\n".join(variants)+"\n]\n",encoding="utf8")
    wire_layouts = {name: source.merged_proto("wireLayout",name) for name in sorted({door["wires"] for door in airlocks.values()})}
    target.write_text(json.dumps({"airlockSettings":airlock_settings,"electrified":electrified,"prying":prying,
                                 "accessLevels": access_levels, "boardConfigurators": board_configurators,
                                 "wireLayouts": wire_layouts, "graphs": graphs, "recipes": entries, "stacks": stacks,
                                 "tiles": tiles, "floorItems": placed_by, "tileTools": tile_tools,
                                 "repairables": repairables, "damageables": damageables, "toolSpeeds": tool_speeds,
                                 "welders": welders, "pipes": pipes, "cables": cables,
                                 "cableObjects": cable_objects, "insulation": insulation,
                                 "computerBoards": computer_boards, "computers": computers,
                                 "machineBoards": machine_boards, "machines": machines, "partTraits": parts,
                                 "doorBoards": door_boards, "inflatables": inflatables, "inflatableItems": inflatable_items, "balloonPoppers": balloon_poppers,
                                 "toolSounds": tools_audio, "barriers": barriers, "rcdDevices":rcd_devices,
                                 "rcdModes":rcd_modes, "rcdAmmo":rcd_ammo, "rcdTargets":rcd_targets}, ensure_ascii=False,
                                 indent=2) + "\n", encoding="utf-8")
    print(f"{len(graphs)} graphs, {len(entries)} recipes")
    dest_audio = build / "assets/ss14/Audio"
    rows = ["# Engineering audio", "", "Unmodified files from space-wizards/space-station-14, `Resources/Audio`. For files without a separate attribution, the SS14 Resources license applies.", "",
            "| File | License | Attribution | Source |", "|---|---|---|---|"]
    for rel, (license, credit, origin) in sorted(audio_rows.items()):
        dst = dest_audio / rel
        dst.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(audio_root / rel, dst)
        rows.append(f"| `{rel}` | {license} | {' '.join(credit.split()).replace('|', '/')} | {origin} |")
    (dest_audio / "ENGINEERING-ATTRIBUTIONS.md").write_text("\n".join(rows) + "\n", encoding="utf8")
    report = ["# Engineering audio not copied", ""]
    if audio_skipped:
        report += ["Original references remain in the catalog; no replacement audio is substituted.", "", "| File | Reason |", "|---|---|"]
        report.extend(f"| `{rel}` | {reason} |" for rel, reason in sorted(audio_skipped.items()))
    else:
        report.append("All referenced engineering audio was copied. No files were skipped.")
    (dest_audio / "ENGINEERING-AUDIO-SKIPPED.md").write_text("\n".join(report) + "\n", encoding="utf8")


if __name__ == "__main__":
    main()
