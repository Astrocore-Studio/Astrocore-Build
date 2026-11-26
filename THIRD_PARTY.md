# Сторонний код и ассеты

Astrocore Build распространяется по [AGPL-3.0-or-later](LICENSE). В репозитории есть материалы
других проектов; их лицензии и авторство сохраняются.

## Код

Системы Build переносятся с C# на Rust по исходникам этих проектов (ревизии — в
[docs/REFERENCE.md](docs/REFERENCE.md)):

| Проект | Лицензия | Правообладатель |
|---|---|---|
| [Space Station 14](https://github.com/space-wizards/space-station-14) | MIT | Copyright (c) 2017-2026 Space Wizards Federation |
| [RobustToolbox](https://github.com/space-wizards/RobustToolbox) | MIT | Copyright (c) 2017-2026 Space Wizards Federation |

Текст лицензии MIT, под которой получен исходный код:

```
Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

Перенесённый модуль называет исходный файл в своём комментарии. Производный код Build
распространяется по AGPL-3.0-or-later с сохранением этого уведомления.

## Ассеты

Спрайты, звуки, карты и прототипы SS14 сохраняют исходные лицензии:

* набор спрайтов RSI — поля `license` и `copyright` в его `meta.json`;
* звуки и отдельные изображения — файл `attributions.yml` в том же каталоге;
* сводка и правила — [assets/ss14/LICENSE-ASSETS.md](assets/ss14/LICENSE-ASSETS.md).

Большинство ассетов — под CC-BY-SA 3.0; часть — под некоммерческими лицензиями
(CC-BY-NC-SA 3.0 и подобными). Проверка наборов спрайтов:

```bash
python3 scripts/check_rsi_licenses.py
```
