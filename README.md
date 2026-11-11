# Astrocore Build

Базовый нейтральный игровой билд на [Astrocore Engine](https://github.com/Astrocore-Studio/Astrocore-Engine):
механики Space Station 14, адаптированные под 3D-мир движка. Тематические игры — форки этого репозитория.

Работа ведётся в ветке `master`. Движок подключён git-зависимостью на его `master`
(точная версия закреплена в `Cargo.lock`).

## Крейты

| Крейт | Назначение |
| --- | --- |
| `prototypes` | Прототипы сущностей: RON, наследование (несколько родителей), адаптер YAML SS14 |
| `assets` | Наборы спрайтов RSI из SS14: поиск, проверка лицензий и файлов состояний |

## Ассеты SS14

`assets/ss14/` — спрайты из space-wizards/space-station-14 под **CC-BY-SA-3.0** (не под лицензией
кода репозитория); подробности и правила — `assets/ss14/LICENSE-ASSETS.md`.

## Проверка

```bash
cargo test --workspace
```
