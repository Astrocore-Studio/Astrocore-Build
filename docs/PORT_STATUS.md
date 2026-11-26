# Состояние переноса

Что из Robust Toolbox и Space Station 14 уже перенесено. Ревизии референса — в
[REFERENCE.md](REFERENCE.md), порядок работ — в [ROADMAP.md](ROADMAP.md). Таблица обновляется
в каждом коммите, который переносит или отключает систему.

Статусы: **готово** — перенесено и покрыто тестами; **частично** — работает основа, остальное
перечислено в примечании; **отложено** — запланировано на указанную задачу плана; **нет** — не
начато.

## Движок (Robust Toolbox → Astrocore Engine)

| Robust | Astrocore | Статус | Примечание |
|---|---|---|---|
| `Robust.Shared/Serialization/Markdown`, `SerializationManager` | `astrocore-data` | готово | Дерево данных, YAML и RON, мост к `serde`, запись в оба формата. |
| `Robust.Shared.Maths` | `astrocore-math` | частично | Углы, направления, `Box2`, цвета. Нет матриц и `Box2Rotated` — придут с физикой (2.5). |
| `EntityManager` (сущности, компоненты, жизненный цикл) | `astrocore-ecs` | готово | Хранилище — `bevy_ecs`. Стадия жизни хранится у сущности, а не у каждого компонента. |
| `EntityEventBus` | `astrocore-ecs::EventBus` | готово | Направленные и широковещательные события, порядок `before/after`. Одна пара «компонент + событие» допускает несколько подписчиков. |
| `ComponentFactory` | `astrocore-ecs::ComponentRegistry` | готово | Имя в данных ↔ тип Rust, разбор и запись компонента. |
| `GameTiming` | `astrocore-ecs::GameTime` | частично | Тики и время симуляции. Нет времени кадра клиента и сглаживания — задача 2.4. |
| `IRobustRandom` | `astrocore-ecs::GameRng` | готово | xoshiro256++ с зерном. |
| `PrototypeManager` | `astrocore-prototypes` | частично | Загрузка, наследование, `CreateVariants`, типизированные виды, `ProtoId<T>`. Отложено: частичные прототипы `!Partial` (в SS14 не используются), категории сущностей (4.4), перезагрузка на лету (2.15). |
| `EntityPrototype`, `EntityManager.Spawn` | `astrocore-prototypes`, `astrocore-map::SpawnWorldExt` | готово | Создание сущности из прототипа с изменениями из файла карты. |
| `TransformComponent`, `SharedTransformSystem` | `astrocore-map::Transform` | частично | Иерархия, мировые координаты, смена родителя, привязка к клетке, этаж `z`. Нет обхода гридов при движении (`GridTraversal`) — задача 2.6. |
| `MapComponent`, `SharedMapSystem.Map/Pause/MapInit` | `astrocore-map::Map` | готово | Карты, пауза, отложенный `MapInit`. |
| `MapGridComponent`, `SharedMapSystem.Grid` | `astrocore-map::MapGrid` | частично | Чанки 16×16 с этажами, тайлы, привязанные сущности, поиск грида по точке. Нет физики грида и разделения грида на части — задачи 2.5 и 5.7. |
| `ITileDefinitionManager` | `astrocore-map::TileDefinitions` | готово | Игровые свойства тайла хранит прототип `tile` в Build (задача 4.1). |
| `Robust.Shared/Containers` | `astrocore-map::ContainerManager` | готово | Контейнеры и слоты, события попытки и результата. |
| `IConfigurationManager` (CVar) | `astrocore-console::Config` | частично | Значения, TOML, события. Нет репликации `REPLICATED` клиентам — задача 2.3. |
| `IConsoleHost` | `astrocore-console::Console` | частично | Команды, права, подсказки. Нет Toolshed и передачи команд по сети — задачи 2.3 и 6.6. |
| `EntityLookupSystem` | — | отложено | Пространственный индекс — вместе с PVS и физикой (2.2, 2.5). |
| `Robust.*/GameStates`, PVS | `astrocore-grid2d` (первая основа) | отложено | Задачи 2.1–2.3. |
| `Robust.Shared/Physics` | — | отложено | Задача 2.5. |
| `Robust.Client` (спрайты, свет, звук, UI, ввод) | — | отложено | Задачи 2.7–2.13. |
| `Robust.Shared/Localization` | — | отложено | Задача 2.14. |
| `Robust.Shared/ContentPack` (ресурсы) | — | отложено | Задача 2.15. |

## Игра (Space Station 14 → Astrocore Build)

Игровые системы пока работают на первой 2D-основе (`2d/`, сценарий Relay Two) и переезжают на
ядро сущностей в фазах 3–6.

| SS14 | Astrocore | Статус | Примечание |
|---|---|---|---|
| `Content.Server/Atmos` (смеси, тайловая атмосфера) | `crates/atmos`, `2d/src/environment.rs` | частично | Работает в Relay Two; перенос на компоненты — задача 5.5. |
| Трубы, насосы, фильтры, вентиляция | `2d/src/piping.rs` | частично | Задача 5.6. |
| `Content.Server/Power` (сети, SMES, подстанции) | `crates/power`, `2d/src/electricity.rs`, `wiring.rs` | частично | Задача 5.4. |
| Урон и лечение | `2d/src/medical.rs` | частично | Задача 3.6. |
| Доступ и ID-карты | `2d/src/access.rs`, `id_console.rs` | частично | Задача 5.2. |
| Должности, лобби, поздний вход | `2d/src/jobs.rs`, `2d/src/lib.rs` | частично | Задача 6.3. |
| Инвентарь и рюкзаки | `2d/src/inventory.rs` | частично | Задачи 3.2–3.4. |
| Перетаскивание | `2d/src/pulling.rs` | частично | Задача 3.8. |
| Остальные системы `Content.*` | — | нет | Фазы 3–7. |

## Данные SS14

Проверено на `Resources/` референса:

| Данные | Результат |
|---|---|
| `Resources/Prototypes` — 2209 файлов | читаются без ошибок: 177 видов, 10 122 прототипа сущностей |
| `Resources/Maps` — 218 файлов, 58 МБ | разбираются в дерево данных и записываются обратно в YAML и RON без расхождений; загрузка в мир — задача 4.1 |
| Компоненты в прототипах сущностей | 1664 типа; в движке зарегистрированы `Transform` и `ContainerContainer`, остальные пропускаются с учётом в отчёте |
