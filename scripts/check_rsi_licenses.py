#!/usr/bin/env python3
"""Проверяет, что у каждого набора спрайтов RSI есть meta.json с лицензией и авторством.

Запуск: python3 scripts/check_rsi_licenses.py [каталог ...]
Без аргументов проверяет assets/ss14/Textures и Resources/Textures, если они есть.
Код возврата 1, если найден набор без meta.json, без поля license или copyright.
"""
import json
import sys
from pathlib import Path


def check(root: Path) -> list[str]:
    problems = []
    for rsi in sorted(p for p in root.rglob("*.rsi") if p.is_dir()):
        meta = rsi / "meta.json"
        if not meta.is_file():
            problems.append(f"{rsi}: нет meta.json")
            continue
        try:
            data = json.loads(meta.read_text(encoding="utf-8-sig"))
        except (OSError, ValueError) as error:
            problems.append(f"{meta}: не читается ({error})")
            continue
        for field in ("license", "copyright"):
            if not str(data.get(field) or "").strip():
                problems.append(f"{meta}: пустое поле {field}")
    return problems


def main() -> int:
    repo = Path(__file__).resolve().parent.parent
    roots = [Path(a) for a in sys.argv[1:]] or [
        p for p in (repo / "assets/ss14/Textures", repo / "Resources/Textures") if p.is_dir()
    ]
    problems = [line for root in roots for line in check(root)]
    for line in problems:
        print(line)
    print(f"Проверено каталогов: {len(roots)}; замечаний: {len(problems)}")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
