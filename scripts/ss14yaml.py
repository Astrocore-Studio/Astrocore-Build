"""Чтение прототипов SS14 (yml) для генераторов содержимого Build."""
import glob
import copy
import yaml


class _Tagged(dict):
    pass


class Loader(yaml.SafeLoader):
    pass


def _tag(loader, suffix, node):
    if isinstance(node, yaml.MappingNode):
        v = _Tagged(loader.construct_mapping(node, deep=True))
    elif isinstance(node, yaml.SequenceNode):
        v = _Tagged(items=loader.construct_sequence(node, deep=True))
    else:
        v = _Tagged(value=loader.construct_scalar(node))
    v['__tag'] = suffix[5:] if suffix.startswith('type:') else suffix
    return v


Loader.add_multi_constructor('!', _tag)


def _merge(dst, src):
    """Поля потомка поверх предка: словари сливаются вглубь, остальное заменяется."""
    for k, v in src.items():
        if isinstance(v, dict) and isinstance(dst.get(k), dict):
            _merge(dst[k], v)
        else:
            dst[k] = copy.deepcopy(v)


class Protos:
    def __init__(self, root):
        self.root = root
        self.by_type = {}
        for f in sorted(glob.glob(root + '/Resources/Prototypes/**/*.yml', recursive=True)):
            try:
                docs = yaml.load(open(f), Loader=Loader)
            except Exception:
                continue
            for e in docs or []:
                if isinstance(e, dict) and isinstance(e.get('type'), str) and isinstance(e.get('id'), str):
                    self.by_type.setdefault(e['type'], {})[e['id']] = e

    def get(self, typ, i):
        return self.by_type.get(typ, {}).get(i)

    def chain(self, typ, i):
        """Предки от самого дальнего к потомку (последний — сам прототип)."""
        e = self.get(typ, i)
        if e is None:
            return []
        out = []
        parents = e.get('parent')
        if parents:
            for p in parents if isinstance(parents, list) else [parents]:
                out.extend(self.chain(typ, p))
        out.append(e)
        return out

    def entity_components(self, i):
        merged = {}
        for e in self.chain('entity', i):
            for c in e.get('components') or []:
                t = c['type']
                cur = merged.setdefault(t, {})
                _merge(cur, c)
        return merged

    def merged_proto(self, typ, i):
        """Прототип с унаследованными полями (для `injectorMode` и подобных)."""
        out = {}
        for e in self.chain(typ, i):
            _merge(out, {k: v for k, v in e.items() if k not in ('parent', 'abstract')})
        return out

    def entity_name(self, i):
        name = None
        for e in self.chain('entity', i):
            if e.get('name'):
                name = e['name']
        return name

    def is_abstract(self, i):
        e = self.get('entity', i)
        return bool(e and e.get('abstract'))
