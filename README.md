<div class="header" align="center">
<img alt="Astrocore Build" width="880" height="300" src="docs/img/logo.svg">
</div>

Astrocore Build — Space Station 14 на Rust: многопользовательская игра про космическую станцию на
[Astrocore Engine](https://github.com/Astrocore-Studio/Astrocore-Engine). Механики, прототипы,
карты, спрайты и звуки переносятся из
[Space Station 14](https://github.com/space-wizards/space-station-14); сервер рассчитан на 160+
игроков.

Сейчас играбелен сценарий Relay Two на первой 2D-основе. Идёт перенос на ядро сущностей и
компонентов по образцу Robust Toolbox.

## Документация

- [docs/ROADMAP.md](docs/ROADMAP.md) — план работ от текущего состояния до финала.
- [docs/REFERENCE.md](docs/REFERENCE.md) — ревизии SS14 и RobustToolbox, с которых идёт перенос.
- [2d/README.md](2d/README.md) — запуск, управление и редактор карт Relay Two.
- [docs/ADR](docs/ADR) — решения Build; решения движка — в
  [Astrocore Engine](https://github.com/Astrocore-Studio/Astrocore-Engine/tree/main/docs/ADR).
- [THIRD_PARTY.md](THIRD_PARTY.md) — лицензии и авторство перенесённого кода и ассетов.

## Запуск

Установите Rust stable. Build подключает Engine из соседнего каталога `../Astrocore-Engine`;
нужную ревизию (файл `engine.rev`) туда кладёт скрипт:

```bash
scripts/fetch_engine.sh
```

Затем из корня Build:

```bash
cargo play
```

Команда собирает 2D-клиент и запускает локальный сервер, который завершится вместе с клиентом.
На Windows доступен `play.cmd`. Отдельный сервер, второй клиент и редактор карт —
`cargo serve2d`, `cargo client2d` и `cargo editor2d`.

## Проверка перед коммитом

```bash
cargo fmt --all -- --check
```

```bash
cargo clippy --locked --workspace --all-targets --features client,prototype-import -- -D warnings
```

```bash
cargo test --locked --workspace --features client,prototype-import
```

## Лицензии

Код этого репозитория, как и [Astrocore Engine](https://github.com/Astrocore-Studio/Astrocore-Engine),
распространяется по лицензии [GNU Affero General Public License v3.0 или новее](LICENSE)
(`AGPL-3.0-or-later`). Если вы распространяете игру или запускаете изменённый сервер как сетевой
сервис, AGPL обязывает предоставить исходный код пользователям, включая ваши изменения.

Ассеты распространяются по лицензиям, указанным в их файлах метаданных. Спрайты в `assets/ss14`
взяты из [Space Station 14](https://github.com/space-wizards/space-station-14) и его форков;
лицензия и авторство каждого набора записаны в его `meta.json` (например,
[лом](assets/ss14/Textures/Objects/Tools/crowbar.rsi/meta.json)). Большинство — под
[CC-BY-SA 3.0](https://creativecommons.org/licenses/by-sa/3.0/). Подробности — в
[assets/ss14/LICENSE-ASSETS.md](assets/ss14/LICENSE-ASSETS.md).

> [!NOTE]
> Часть ассетов распространяется по некоммерческим лицензиям
> [CC-BY-NC-SA 3.0](https://creativecommons.org/licenses/by-nc-sa/3.0/) и подобным. Если вы хотите
> использовать проект в коммерческих целях, их нужно удалить или заменить.
