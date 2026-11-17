# Спрайты Space Station 14 (RSI)

Каталог `Textures/` скопирован из репозитория
[space-wizards/space-station-14](https://github.com/space-wizards/space-station-14)
(коммит `910154a7a8fcc6e4fc24f7c5b4925e405d74cd3e`, путь `Resources/Textures/`) без изменений.

Лицензия и авторство **каждого** набора указаны в его `meta.json` (поля `license` и `copyright`);
в каталоге присутствуют разные исходные лицензии. Эти файлы **не** покрываются лицензией
кода репозитория (`LICENSE` в корне): для каждого ресурса сохраняется его исходная лицензия.

2026-11-15 владелец проекта разрешил перенос всех исходных ассетов SS14, включая NC и Custom
(ответ «можно все»). Это решение снимает внутренний запрет на импорт; исходные лицензии,
авторство и ссылки на источники сохраняются. Отдельного согласования каждого NC/Custom-файла не требуется.

Правила:

* набор нельзя править без сохранения `meta.json` с лицензией и авторством;
* производные (перерисовка, смена размера, склейка в атлас) перечисляются здесь с исходной
  лицензией и авторством соответствующего набора;
* набор без `license` или `copyright` в `meta.json` копируется, но `sync-sprites` перечисляет такие
  наборы в отчёте: перед выпуском их стоит проверить.

## Наборы

| Набор | Лицензия |
| --- | --- |
| `Mobs/Species/Human/parts.rsi` | см. `meta.json` |
| `Objects/Storage/boxes.rsi` | CC-BY-SA-3.0 |
| `Objects/Tools/crowbar.rsi` | CC-BY-SA-3.0 |
| `Objects/Tools/screwdriver.rsi` | CC-BY-SA-3.0 |
| `Objects/Tools/wrench.rsi` | CC-BY-SA-3.0 |
| `Structures/Walls/solid.rsi` | см. `meta.json` |
| `Structures/Doors/Airlocks/Standard/airlock-base.rsi` | CC-BY-SA-4.0 |
| `Clothing/Back/Backpacks/backpack.rsi` | CC-BY-SA-3.0 |
| `Objects/Misc/pens.rsi` | CC-BY-SA-3.0 |
| `Tiles/Basic/Dark/dark.png` | CC0-1.0 (см. `attributions.yml` рядом) |

`render-manifest.json` — служебный файл (соответствие идентификаторов и картинок), собирается из прототипов.

Наборы проводки `Structures/Power/Cables/{hv,mv,lv}_cable.rsi` и альтернативных слоёв труб `Structures/Piping/Atmospherics/pipe_alt{1,2}.rsi` взяты из снимка SS14 `31f782a809851c674bd8f86c3aca7c381118c539`. Авторство и условия каждого набора сохранены в оригинальном `meta.json`.

## Интерфейс (`Textures/Interface`)

Слоты, рамки хранилища и значки руки скопированы из `Resources/Textures/Interface/` того же коммита
без изменений. Лицензия указана в `Interface/Default/meta.json`: **CC-BY-NC-SA-3.0** (Goonstation,
слегка изменено PixelTK, слоты ID — YoungThugSS14). Это **некоммерческая** лицензия: для
коммерческого распространения эти файлы нужно заменить собственными.


## Инженерные конструкции и звук

Состояния каркасов, усиленной стены, решёток и окон в `Structures/Machines/{parts,computers}.rsi`, `Structures/Walls/{solid-reinforced,grille}.rsi` и `Structures/Windows/window-*.rsi` скопированы из SS14. Полные записи лицензии и авторства сохранены в каждом исходном `meta.json`. Оригинальные OGG, записи об авторстве и условиях использования перечислены в [Engineering audio](Audio/ENGINEERING-ATTRIBUTIONS.md).

Стеклянный шлюз, настил Catwalk, слои труб и кабели используют исходные RSI со своими записями в `meta.json`. Иконки RCD и кнопки его кругового меню перенесены из `Resources/Textures/Interface/Radial` эталонного SS14 без изменений.

Ведомственные шлюзы Saltern, дополнительные материалы стен и атмосферные устройства используют полные исходные RSI. Авторство и лицензии сохранены в каждом `meta.json`. Файлы копируются из локального эталона SS14 без обработки изображений.

Контакты, провода, их обрезанные состояния и лампы в `Textures/Interface/WireHacking` скопированы целиком из `Resources/Textures/Interface/WireHacking` того же снимка. SVG, исходные PNG и параметры выборки сохранены без изменений; применяется лицензия ресурсов SS14 CC-BY-SA-3.0.
