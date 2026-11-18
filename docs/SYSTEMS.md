# Игровые системы Build

ADR: [ADR-0001](ADR/0001-separation-from-ss14-and-workflow.md). Код: `crates/gameplay/src/systems/`.

Механика — **система**: тип, реализующий `GameSystem` (`systems/mod.rs`). У неё своё состояние,
свои команды клиента (`commands()` + `command()`), свой тик (`tick()`, после общей симуляции), щелчок пустой рукой (`interact_hand()`, раньше общих
обработчиков), применение предмета к предмету (`interact_using()`), отключение игрока (`player_left()`) и
свои реплицируемые компоненты. Общее состояние мира — `State` (персонажи `actors`, предметы,
должности, лобби, токены игроков); система получает его по ссылке и не хранит чужих данных.

## Включение, отключение, замена

```rust
GameplayModule::new(registry)?
    .without_system("chat")                // отключить
    .with_system(Box::new(MyChat::default())) // своя система с id "chat" заменяет стандартную
```

`GameplayModule::system_ids()` — какие системы включены. Команду клиента разбирает первая
система, у которой она в `commands()`; остальное разбирает модуль по-старому.

## Как добавить систему

1. Файл `systems/<имя>.rs`: тип системы со своим состоянием, `impl GameSystem` (`id`, `commands`,
   `command`, `tick`, `as_any_mut`), константы команд и компонентов, типы вида для клиента.
2. Зарегистрировать в `Systems::defaults` (`systems/mod.rs`) и компоненты — в
   `wire::register_replication`.
3. Данные — в прототипах RON (`content/prototypes`), читаются при создании системы из реестра.
4. Тесты — в `tests.rs` на стенде сервера (`harness`, `harness_with(|m| …)`): команда клиента →
   тики → проверка реплицированного компонента у клиента.
5. Клиентская часть — в `apps/client`, по реплицированным компонентам.

## Состояние переноса

| Система | Где | Команды | Компоненты |
| --- | --- | --- | --- |
| Чат (`chat`) | `systems/chat.rs` | `chat` | `build.chat` |
| Консоль и права (`console`) | `systems/console.rs`, `admin.rs` | `console.exec` | `build.console`, `build.admin` |
| Настройка плат дверей (`door_electronics`) | `door_electronics.rs` | `door_electronics.update`, `door_electronics.close`, настройщик по плате | `build.door_electronics` |
| Мониторинг питания (`power_monitor`) | `power_monitor.rs` | `power.focus`, `power.close`, щелчок по консоли | `build.powermonitor*` |

Ещё в `State` (переносятся по одной, с зелёными тестами на каждом шаге): лобби и раздача
должностей, персонажи и здоровье, руки и инвентарь, хранилища, бой и оружие, химия
(`chemlab`, раздатчики, шприцы), медицина (`cryo`, дефибриллятор), атмосфера и сигнализации
(`alerts`), питание, двери (`airlock_doors`, `airlock_wires`,
строительство (`construction`, `computer_construction`, `rcd`, `floor_tiles`,
`material_stacks`, `barriers`, `inflatables`, `repairable`), торговые автоматы, утилизация,
конвейеры, раунд и его конец. Механики, у которых уже свой файл с `impl State`, переносятся
первыми: им нужно лишь забрать своё состояние из `State` и стать `GameSystem`.
