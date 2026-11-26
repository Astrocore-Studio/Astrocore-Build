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
| `Robust.Shared.Maths` | `astrocore-math` | готово | Углы, направления, `Box2`, цвета; в сети хранятся числами. Матрицы не нужны: положение считается поворотом и сдвигом. |
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
| `Robust.Server/GameStates`, `Robust.Client/GameStates` | `astrocore-net` | частично | Состояния компонентов от подтверждённого тика, сессии, `Actor`, сетевые события, связь с Lightyear. Отложено: сжатие больших состояний, маски видимости (призраки), репликация CVar, сглаживание чужих сущностей между состояниями. |
| `PvsSystem`, `PvsOverrideSystem` | `astrocore-net` (PVS) | частично | Клетки 8×8 тайлов по этажам, бюджет новых сущностей, принудительная видимость. Отложено: видимость соседних этажей через проёмы, несколько глаз у сессии. |
| `EntityLookupSystem` | `astrocore-physics::PhysicsWorldExt::entities_intersecting` | частично | Поиск тел в прямоугольнике. Лучи и поиск по радиусу — задача 3.1. |
| `Robust.Shared/Physics` | `astrocore-physics` | частично | Тела, фикстуры, слои и маски SS14, события касаний, сон тел. Упрощено: без трения между телами, вращения от ударов и сочленений (3.8); столкновения только внутри одного грида и этажа (шаттлы — 7.8). |
| `SharedMoverController` (ядро), ввод движения | `astrocore-movement` | частично | Скорость по `HandleMobMovement`, предсказание своего персонажа. В игре (фаза 3): невесомость, трение тайлов, шаги, передача управления. |
| Предсказание Robust (откат и повтор тиков) | `astrocore-movement::Prediction` | частично | Только движение своего персонажа (решение владельца); остальные системы — по мере переноса. |
| `SpriteComponent`, `SpriteSystem`, RSI | `astrocore-client::Sprite`, `Textures` | частично | Слои, состояния, направления 1/4/8, анимация, глубина отрисовки, цвет, смещение, масштаб. Нет шейдеров слоёв и `Appearance` (2.8). |
| Отрисовка тайлов грида | `astrocore-client` (`view`) | частично | Картинка на чанк, варианты тайлов. Нет декалей и краёв тайлов. |
| `EyeComponent`, камера | `astrocore-client::Eye` | частично | Следует за сущностью игрока, 15 тайлов по вертикали. Нет зума и поворота. |
| Ввод | `astrocore-client` | частично | Клавиши движения. `KeyFunctions`, `keybinds.yml`, клики по сущностям — задача 2.13. |
| Свет и поле зрения, звук, UI | — | нет | Задачи 2.10–2.12. |
| `MapLoaderSystem`, `EntityDeserializer` | `astrocore-map::MapLoadWorldExt` | частично | Карты SS14 форматов 6–7: тайлы, сущности, иерархия, переименования из `migration.yml`. Нет сохранения мира в файл. |
| `Robust.Shared/Localization` | `astrocore-loc` | частично | Fluent, запасной язык, атрибуты `ent-ID`. Нет грамматических функций SS14 (`THE`, `GENDER`, …) — задача игры; 13 сообщений en-US с расширением синтаксиса Linguini не разбираются. |
| `Robust.Shared/ContentPack` (ресурсы), RSI | `astrocore-resources` | частично | Каталоги контента с приоритетом, описания RSI. Нет архивов и перезагрузки на лету. |

## Игра (Space Station 14 → Astrocore Build)

Игровые системы пока работают на первой 2D-основе (`2d/`, сценарий Relay Two) и переезжают на
ядро сущностей в фазах 3–6. Игра на ядре сущностей живёт в каталоге `game/`.

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
| `Content.Client/IconSmoothing` | `game/src/smooth.rs` | готово | Все правила SS14: `Corner`, `CardinalCorner`, `Diagonal`, `AltDiagonal`, `Direction`, `Edge`. Нет шейдера слоёв и `RandomIconSmooth`. |
| `MarkerComponent`, `MarkerSystem` | `game/src/client.rs` | частично | Метки скрыты; показа для мапперов нет. |
| `CollisionGroup`, `DrawDepth` | `game/src/content.rs` | готово | Таблицы имён SS14. |
| Вход игрока на карту | `game/src/server.rs` | частично | Тело из `game.player_prototype` у точки появления. Должности и лобби — задача 6.3. |
| Остальные системы `Content.*` | — | нет | Фазы 3–7. |

## Данные SS14

Проверено на `Resources/` референса:

| Данные | Результат |
|---|---|
| `Resources/Prototypes` — 2209 файлов | читаются без ошибок: 177 видов, 10 122 прототипа сущностей |
| `Resources/Maps` — 218 файлов, 58 МБ | разбираются в дерево данных и записываются обратно в YAML и RON без расхождений; Saltern загружается в мир целиком: 11 484 сущности и 6905 тайлов |
| `Resources/Textures` — 3199 наборов RSI, 24 170 состояний | описания разбираются без замечаний, картинка есть у каждого состояния |
| `Resources/Locale/en-US` — 1242 файла | 28 199 сообщений; 13 не разбираются (расширение синтаксиса) |
| Компоненты в прототипах сущностей | 1664 типа; движок понимает `Transform`, `ContainerContainer`, `Physics`, `Fixtures`, `InputMover`, `MovementSpeedModifier`, `Sprite`, игра — `IconSmooth` и `Marker`, остальные пропускаются с учётом в отчёте |
