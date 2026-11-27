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
| `TransformComponent`, `SharedTransformSystem`, `SharedGridTraversalSystem` | `astrocore-map::Transform`, `GridTraversal` | готово | Иерархия, мировые координаты, смена родителя, привязка к клетке, этаж `z`, переход между гридами на ходу. |
| `MapComponent`, `SharedMapSystem.Map/Pause/MapInit` | `astrocore-map::Map` | готово | Карты, пауза, отложенный `MapInit`. |
| `MapGridComponent`, `SharedMapSystem.Grid` | `astrocore-map::MapGrid` | частично | Чанки 16×16 с этажами, тайлы, привязанные сущности, поиск грида по точке. Нет физики грида и разделения грида на части — задачи 2.5 и 5.7. |
| `ITileDefinitionManager` | `astrocore-map::TileDefinitions` | готово | Игровые свойства тайла хранит прототип `tile` в Build (задача 4.1). |
| `Robust.Shared/Containers` | `astrocore-map::ContainerManager` | готово | Контейнеры и слоты, события попытки и результата. |
| `IConfigurationManager` (CVar) | `astrocore-console::Config` | частично | Значения, TOML, события. Нет репликации `REPLICATED` клиентам — задача 2.3. |
| `IConsoleHost`, `ServerConsoleHost`, `ClientConsoleHost` | `astrocore-console::Console`, `astrocore-net` (`RemoteCommand`) | частично | Команды, права, подсказки, выполнение команд игрока на сервере. Нет Toolshed (6.6). |
| `Robust.Server/GameStates`, `Robust.Client/GameStates` | `astrocore-net` | частично | Состояния компонентов от подтверждённого тика, сессии, `Actor`, сетевые события, связь с Lightyear, сжатие больших состояний (LZ4). Отложено: передача настроек сервера клиенту (6.3). |
| `PvsSystem`, `PvsOverrideSystem`, `VisibilityComponent`, `EyeComponent.VisibilityMask` | `astrocore-net` (PVS, `Visibility`, `Eye`) | частично | Клетки 8×8 тайлов по этажам, бюджет новых сущностей, принудительная видимость, слои видимости и маска глаза. Отложено: видимость соседних этажей через проёмы, несколько глаз у сессии. |
| `EntityLookupSystem`, лучи `SharedPhysicsSystem` | `astrocore-physics` (`entities_intersecting`, `RayWorldExt`, `DistanceWorldExt`) | частично | Поиск тел в прямоугольнике, лучи с маской, расстояние до фигур. Нет поиска по радиусу с флагами `LookupFlags`. |
| `Robust.Shared/Physics`, `SharedJointSystem` | `astrocore-physics` | частично | Тела, фикстуры, слои и маски SS14, события касаний, сон тел, трение между телами, вращение от ударов, дистанционные сочленения (`DistanceJoint`). Упрощено: сочленения без вращения тел, других видов сочленений нет; столкновения только внутри одного грида и этажа (шаттлы — 7.8). |
| `SharedMoverController` (ядро), ввод движения | `astrocore-movement` | частично | Скорость по `HandleMobMovement`, предсказание своего персонажа. В игре (фаза 3): невесомость, трение тайлов, шаги, передача управления. |
| Предсказание Robust (откат и повтор тиков) | `astrocore-movement::Prediction` | частично | Только движение своего персонажа (решение владельца); остальные системы — по мере переноса. |
| `SpriteComponent`, `SpriteSystem`, RSI | `astrocore-client::Sprite`, `Textures` | частично | Слои, состояния, направления 1/4/8, анимация, глубина отрисовки, цвет, смещение, масштаб; движение сглаживается между тактами. Нет шейдеров слоёв. |
| `AppearanceComponent`, `GenericVisualizer` | `astrocore-net::Appearance`, `astrocore-client::GenericVisualizer` | готово | Значения внешнего вида по ключам от сервера, смена слоёв по данным прототипа. Визуализаторы с кодом переносятся вместе с системами игры. |
| Отрисовка тайлов грида | `astrocore-client` (`view`) | частично | Картинка на чанк, варианты тайлов. Нет декалей и краёв тайлов. |
| `EyeComponent`, камера | `astrocore-client::Eye` | частично | Следует за сущностью игрока, 15 тайлов по вертикали. Нет зума и поворота. |
| `InputManager`, `keybinds.yml`, контексты, `InputCmdMessage` | `astrocore-input`, `astrocore-client` (`input`) | готово | Назначения SS14, сочетания, приоритеты, переназначение, команды ввода с точкой и сущностью под курсором (по пикселям спрайта). Обработчики функций пишет игра. |
| `PointLightComponent`, `OccluderComponent`, свет и FOV `Clyde` | `astrocore-light`, `astrocore-client` (`lighting`) | частично | Свет с тенями и поле зрения по клеткам; карта освещения с шагом 1/8 тайла; маски направленного света (фонарик); свет в закрытых контейнерах гаснет. Нет мягких теней (8.9). |
| `Robust.Shared/Audio`, `AudioSystem` | `astrocore-audio`, `astrocore-client` (`audio`) | частично | Звуки по пути и из коллекций, рассылка по слышимости, затухание, фоновые `AmbientSound`. Нет стереопанорамы и эффектов (эхо). |
| `SharedUserInterfaceSystem`, `BoundUserInterface` | `astrocore-ui` | готово | Открытие окна игроку, состояние, сообщения, закрытие по расстоянию. |
| `Robust.Client/UserInterface` (окна, элементы) | `astrocore-client` (`windows`) на `bevy_ui` | частично | Рамка окна, перетаскивание, закрытие, кнопки и текст. Остальные элементы (списки, поля ввода, вкладки) добавляются с окнами игры. |
| `MapLoaderSystem`, `EntityDeserializer`, `EntitySerializer` | `astrocore-map::MapLoadWorldExt`, `MapSaveWorldExt` | готово | Карты SS14 форматов 6–7: загрузка под заданным номером, сохранение неинициализированной карты в формат 7 отличиями от прототипов; непонятые компоненты и поля сохраняются как были, номера сущностей и тайлов из файла сохраняются. Saltern пересохраняется без потерь. |
| `PlacementManager` (сервер и клиент) | `game/src/placement.rs`, `mapping_ui.rs` | частично | Размещение сущностей, тайлов, декалей, ластик, привязка к клеткам, линии и прямоугольники, поворот, копирование. Нет режимов выравнивания `AlignWall`/`AlignSimilar`. |
| Края тайлов (`EdgeSprites`, `Clyde.GridRendering`) | `astrocore-map::TileDefinitions`, `astrocore-client` | частично | Края на клетках существующих чанков. |
| `Robust.Shared/Localization` | `astrocore-loc` | частично | Fluent, запасной язык, названия и описания сущностей из `ent-ID` с наследованием от родителей. Грамматические функции SS14 (`THE`, `GENDER`, …) — в игре (`game/src/grammar.rs`); 13 сообщений en-US с расширением синтаксиса Linguini не разбираются. |
| `Robust.Shared/ContentPack` (ресурсы), RSI | `astrocore-resources` | частично | Каталоги контента с приоритетом, описания RSI. Нет архивов и перезагрузки на лету. |

## Игра (Space Station 14 → Astrocore Build)

Игровые системы пока работают на первой 2D-основе (`2d/`, сценарий Relay Two) и переезжают на
ядро сущностей в фазах 3–6. Игра на ядре сущностей живёт в каталоге `game/`.

| SS14 | Astrocore | Статус | Примечание |
|---|---|---|---|
| `SharedDoorSystem`, `SharedAirlockSystem`, `DoorBoltSystem`, `PryingSystem`, `WeldableSystem`, `FirelockSystem` | `game/src/doors.rs`, `firelock.rs`, `weldable.rs`, `door_visuals.rs` | частично | Двери, шлюзы, болты, аварийный доступ, взлом, сварка, автозакрытие, пожарные шлюзы по тревоге и перепаду. Нет топлива сварки (7.2), стыковочных шлюзов (7.8), вида огней пожарного шлюза на клиенте. |
| `AccessReaderSystem`, `IdCardSystem`, `IdCardConsoleSystem`, `LockSystem` | `game/src/access.rs`, `id_console.rs`, `lock.rs` | частично | Уровни доступа из прототипов, ID-карты и ПДА, консоль ID-карт, замки шкафов и ящиков. Ключи записей станции — с записями (фаза 6). |
| `WiresSystem` и действия проводов | `game/src/wires.rs`, `wires_ui.rs` | частично | Панели, провода по прототипам, кусачки и мультитул, действия дверей и питания. Нет провода «паника» воздушной тревоги (7.9). |
| `PowerNetSystem`, `NodeGroupSystem`, `ApcSystem`, `BatterySystem`, кабели | `game/src/power.rs`, `node_container.rs`, `cables.rs`, `cable_visuals.rs`, `power_ui.rs` | частично | Сети HV/MV/LV, решатель, SMES, подстанции, APC с окном, потребители, удар током. Сеть узлов пересобирается целиком (долг — 8.1). |
| `AmeControllerSystem` и двигатели | `game/src/ame.rs` | частично | AME с топливом и окном. Солнечные панели, ТЭГ и Тесла — после фазы 6. |
| `AtmosphereSystem` (смеси, тайловая атмосфера, `Monstermos`, огонь, реакции), `AirtightSystem`, `BarotraumaSystem` | `game/src/atmos/`, `airtight.rs`, `barotrauma.rs`, `gas_overlay_visuals.rs` | частично | Газы и реакции из прототипов, разгерметизация, огонь, баротравма, газовый слой на клиенте. Взрыв и атмосфера — только на гридах. |
| Трубы и атмосферные устройства (`PipeNet`, насосы, клапаны, фильтры, вентиляции, скрубберы) | `game/src/pipes.rs`, `atmos_devices.rs` | частично | Нет смесителей, радиаторов, теплообменников. |
| Канистры, порты, термомашины, баллоны, газоанализатор | `game/src/atmos_portables.rs`, `gas_tank.rs`, `gas_analyzer.rs` | частично | Нет космонагревателя, вида прочности баллона. |
| `TemperatureSystem`, `FlammableSystem`, `RespiratorSystem`, `LungSystem`, `InternalsSystem` | `game/src/temperature.rs`, `flammable.rs`, `respiration.rs`, `internals.rs`, `fire_visuals.rs` | частично | Температура тел, горение, дыхание, баллон через маску (ADR-0009). Нет значков тревог (7.1), тушения (7.2), реактивных ранцев. |
| `AtmosMonitorSystem`, `AtmosAlarmableSystem`, `AirAlarmSystem`, `FireAlarmSystem` | `game/src/atmos_alarms.rs`, `alarm_visuals.rs` | частично | Датчики, тревоги, режимы воздушной тревоги с окном, порты сигналов. Нет взлома электромагнитом (7.7), пульта тревог станции. |
| `DeviceNetworkSystem`, `DeviceListSystem` | `game/src/device_network.rs` | частично | Сети по номеру, адреса, частоты, проводная, беспроводная и APC, списки из карты (ADR-0010). Нет настройщика сети (6.6), маршрутизаторов, глушилок. |
| `DeviceLinkSystem`, `DoorSignalControlSystem`, `SignalSwitchSystem`, `TwoWayLeverSystem`, `LogicGateSystem` | `game/src/device_linking.rs`, `signal_devices.rs` | частично | Порты, связи из карты, импульсы и уровни, перегрузка, двери, кнопки, рычаги, логические элементы. Нет таймеров (7.3), конвейеров (7.5), настройщика (6.6). |
| `ConstructionSystem`, `FloorTileSystem`, `AnchorableSystem` | `game/src/construction.rs`, `construction_ui.rs`, `floor_tile.rs`, `anchorable.rs` | частично | Графы конструкций SS14, меню строительства, плитки, закрепление. Неперенесённые действия графов пропускаются. |
| `PoweredLightSystem`, `GravityGeneratorSystem` | `game/src/lights.rs`, `gravity.rs`, `light_visuals.rs` | частично | Светильники с лампами и сигналами, генератор гравитации, невесомость. Нет тряски грида, магнитных ботинок. |
| Сингулярность, сдерживающее поле, эмиттеры, ускоритель частиц, излучение | `game/src/singularity.rs`, `containment.rs`, `particle_accelerator.rs`, `radiation.rs` | частично | Нет искажения пространства, счётчика Гейгера, проводов пульта ускорителя. |
| `ExplosionSystem` | `game/src/explosion.rs`, `explosion_visuals.rs` | частично | Интенсивность по клеткам, обработка по тактам, урон и разрушение, вид огня. Нет нескольких гридов и открытого космоса (7.8). |
| Атмос, трубы, питание, доступ (Relay Two) | `crates/atmos`, `crates/power`, `2d/src/` | заменено | В игре на ядре — системы выше; Relay Two не переносится (решение владельца, 4.5). |
| Должности, лобби, поздний вход | `2d/src/jobs.rs`, `2d/src/lib.rs` | частично | Задача 6.3. |
| `Content.Client/IconSmoothing` | `game/src/smooth.rs` | готово | Все правила SS14: `Corner`, `CardinalCorner`, `Diagonal`, `AltDiagonal`, `Direction`, `Edge`. Нет шейдера слоёв и `RandomIconSmooth`. |
| `MarkerComponent`, `MarkerSystem` | `game/src/client.rs` | частично | Метки скрыты; показа для мапперов нет. |
| `CollisionGroup`, `DrawDepth`, `VisibilityFlags` | `game/src/content.rs` | готово | Таблицы имён SS14. |
| Вход игрока на карту | `game/src/server.rs` | частично | Человек (`MobHuman`, настройка `game.player_prototype`) у точки появления. Должности и лобби — задача 6.3. |
| `ActivatableUISystem` | `game/src/activatable_ui.rs` | частично | Открытие окна устройства «использовать», только в руке, одним пользователем, глагол. Нет проверки питания (`ActivatableUIRequiresPower`). |
| `ContentContexts` (контексты ввода) | `game/src/input_contexts.rs` | готово | Таблица получена из исходника SS14. |
| `SharedInteractionSystem` (клик, клик предметом, «использовать», дальность и преграды, доступ сквозь контейнеры) | `game/src/interaction.rs` | готово | Настенные предметы, обход дальности по тегу, поворот к цели. Нет ретрансляции управления (`InteractionRelay`). |
| `ActionBlockerSystem` | `game/src/action_blocker.rs` | готово | События попытки: сознание, взаимодействие, использование, подбор, выброс, бросок, движение, удар. |
| `TagSystem`, `EntityWhitelistSystem` | `game/src/tag.rs`, `whitelist.rs` | готово | Непереносённые компоненты в белых списках проверяются по прототипу сущности (ADR-0005). |
| `SharedHandsSystem`, `ItemComponent`, `ThrowingSystem`, `TileFrictionController` | `game/src/hands.rs`, `item.rs`, `throwing.rs`, `friction.rs` | частично | Руки, подбор, выброс, бросок с полётом и приземлением, трение тайлов. Нет смещений спрайтов в руках (`displacement`). |
| `InventorySystem`, `ClothingSystem` | `game/src/inventory.rs`, `clothing_visuals.rs`, `inventory_hud.rs` | частично | Слоты по шаблону, одевание с задержкой, звуки, вид одежды на теле. Нет снятия с другого (`Stripping`), маски для женского силуэта. |
| `SharedStorageSystem`, `SharedEntityStorageSystem` | `game/src/storage.rs`, `entity_storage.rs`, `storage_hud.rs` | частично | Сетка с формами и поворотами, окно рюкзака, шкафы и ящики. Нет быстрой укладки с пола (`AreaInsert`), вложенных окон. Замки — в `lock.rs`. |
| `ExamineSystem`, `SharedVerbSystem`, `SharedPopupSystem`, `SharedDoAfterSystem` | `game/src/examine.rs`, `verbs.rs`, `popups.rs`, `do_after.rs`, `overlays.rs` | готово | Осмотр с подробностями по дальности, контекстное меню, всплывающие сообщения, действия с прогрессом и отменой. |
| `DamageableSystem`, `MobStateSystem`, `MobThresholdSystem`, `StandingStateSystem` | `game/src/damage.rs`, `mobs.rs`, `standing.rs`, `mob_visuals.rs` | частично | Урон в сотых, типы, группы, контейнеры, сопротивления, пороги из данных, крит и смерть, лёжа и стоя, спрайт мёртвого. Нет брони одежды и вспышки урона (7.3), значков здоровья (7.1). |
| `HealingSystem`, `DamageOtherOnHitSystem` | `game/src/healing.rs`, `damage_other_on_hit.rs` | частично | Лечение предметами стопками, урон от брошенного. Кровь — 7.1. |
| `SharedStackSystem` | `game/src/stack.rs`, `stack_visuals.rs` | готово | Число, слияние, разделение, вид по числу. |
| `SharedToolSystem` | `game/src/tools.rs` | частично | Качества, действие с задержкой, мультитул, плитки (`floor_tile.rs`). Топливо сварки — 7.2. |
| `PullingSystem`, `PullController`, `SharedVirtualItemSystem` | `game/src/pulling.rs`, `virtual_item.rs` | частично | Тяга на сочленении, занятая рука, подтаскивание к точке. Нет значков-тревог (7.1). |
| `SharedActionsSystem` | `game/src/actions.rs`, `action_bar.rs` | частично | Действия без цели, на сущность и на точку, перезарядка, горячая панель. Нет действий с задержкой (`DoAfterArgs`), повышения уровня действий, перетаскивания по ячейкам. |
| `SharedCombatModeSystem`, `SharedMeleeWeaponSystem` | `game/src/combat.rs` | частично | Режим боя действием, лёгкий удар оружием, перчатками или кулаком. Остальное — 7.3. |
| `BodySystem`, `InitialBodySystem`, `HandOrganSystem`, `VisualBodySystem` | `game/src/body.rs`, `body_visuals.rs` | частично | Тело из органов, руки от кистей, цвет кожи вида. Нет меток внешности и профиля (6.2). |
| Команды карт (`MapCommands`, `MapPausingCommands`, `TeleportCommands`, `MappingCommand`, `AGhostCommand`) | `game/src/map_commands.rs`, `dev_console.rs` | частично | `mapping`, `loadmap`, `savemap`, `addmap`, `rmmap`, `lsmap`, пауза, `mapinit`, `tp`, `tpto`, `tpgrid`, `aghost`; консоль на «~». Права — только игрок с машины сервера до 6.6. |
| `Decal`, `DecalChunkComponent`, `DecalGridComponent`, `DecalOverlay` | `game/src/decals.rs`, `decal_visuals.rs` | частично | Чтение обоих форматов, отрисовка, размещение и удаление, сохранение в формат SS14 (ADR-0006). Нет выбора цвета и очистки декалей (`cleanable`). |
| Урон и лечение (Relay Two) | `2d/src/medical.rs` | заменено | В игре на ядре — системы урона и лечения выше; Relay Two не переносится (решение владельца, 4.5). |
| Инвентарь, рюкзаки, перетаскивание (Relay Two) | `2d/src/inventory.rs`, `pulling.rs` | заменено | В игре на ядре — системы выше. |
| Остальные системы `Content.*` | — | нет | Фазы 6–7. |

## Данные SS14

Проверено на `Resources/` референса:

| Данные | Результат |
|---|---|
| `Resources/Prototypes` — 2209 файлов | читаются без ошибок: 177 видов, 10 122 прототипа сущностей |
| `Resources/Maps` — 218 файлов, 58 МБ | разбираются в дерево данных и записываются обратно в YAML и RON без расхождений; Saltern загружается в мир целиком: 11 484 сущности и 6905 тайлов |
| `Resources/Textures` — 3199 наборов RSI, 24 170 состояний | описания разбираются без замечаний, картинка есть у каждого состояния |
| `Resources/Locale/en-US` — 1242 файла | 28 199 сообщений; 13 не разбираются (расширение синтаксиса) |
| Компоненты в прототипах сущностей | 1664 типа; движок понимает `Transform`, `ContainerContainer`, `Physics`, `Fixtures`, `InputMover`, `MovementSpeedModifier`, `Sprite`, `Appearance`, `GenericVisualizer`, `PointLight`, `Occluder`, `AmbientSound`, `UserInterface`, `Clickable`, игра — системы фазы 3 (руки, инвентарь, хранилища, урон, стопки, инструменты, действия, тело и другие, всего 54 компонента); все 10 127 прототипов сущностей создаются без ошибок, остальные компоненты пропускаются с учётом в отчёте |
