# Референс переноса

Astrocore переносит Space Station 14 и его движок на Rust. Поведение сверяется с этими ревизиями:

| Проект | Репозиторий | Ревизия |
|---|---|---|
| Space Station 14 | [space-wizards/space-station-14](https://github.com/space-wizards/space-station-14) | `2f4a2aad293c1e43f2a2ca673333f7dddb2b9b6d` |
| RobustToolbox | [space-wizards/RobustToolbox](https://github.com/space-wizards/RobustToolbox) | `1afd6929b473dd537b42cb668a961c84a2e2e7c6` |

RobustToolbox — подмодуль SS14; его ревизия задана ревизией SS14.

## Получить исходники

```bash
scripts/fetch_reference.sh
```

Скрипт кладёт оба проекта в `../reference/space-station-14` (рядом с чекаутом Build, вне
репозитория).

## Правила

* В описании коммита и в комментарии модуля указывается исходный файл с ревизией, например
  `Robust.Shared/GameObjects/EntityManager.cs @ 1afd692`.
* Переносится поведение, а не синтаксис C#.
* Ревизии обновляются отдельным коммитом. В его описании перечисляются изменения upstream в уже
  перенесённых системах (сверка по путям из `docs/PORT_STATUS.md`).
* Лицензии и авторство перенесённого кода и ассетов — в [THIRD_PARTY.md](../THIRD_PARTY.md).
