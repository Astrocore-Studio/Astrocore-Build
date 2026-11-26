<div class="header" align="center">
<img alt="Astrocore Build" width="880" height="300" src="docs/img/logo.svg">
</div>

Astrocore Build — многопользовательская 2D-станционная песочница на Rust на
[Astrocore Engine](https://github.com/Astrocore-Studio/Astrocore-Engine).
Устройство игровых систем и сценарии NightCity служат референсом при
переносе механик на движок Astrocore.

## Запуск

Установите Rust stable и откройте чекауты Build и Engine в соседних каталогах
`Astrocore-Build` и `Astrocore-Engine`. Выполните из корня Build:

```bash
cargo play
```

Команда собирает 2D-клиент и запускает локальный сервер, который завершится
вместе с клиентом. На Windows из этого же каталога доступен `play.cmd`.

Для отдельного сервера, подключения второго клиента и редактирования карт
есть команды `cargo serve2d`, `cargo client2d` и `cargo editor2d`.
[Параметры запуска, управление и сценарий Relay Two](2d/README.md)

## Сборка и проверка

```bash
cargo build --locked --bin astrocore-2d-server
cargo build --locked --features client
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets --features client -- -D warnings
cargo test --locked --workspace --features client
```

В корневом workspace находятся 2D-сервер, клиент и редактор, а также
библиотеки атмосферы и питания. Старые документы в `docs/` фиксируют
историю 3D-этапа и переноса механик. Актуальные команды и формат карт
описаны в документации Relay Two.

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
