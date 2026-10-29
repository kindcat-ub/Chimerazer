# Chimerazer

Неофициальный open-source конфигуратор периферии Razer для Linux. Настраивает устройство, записывает параметры в его память и не держит фоновых процессов.

Статус: **M4 принят с исключением неисправной прокрутки**. Naga Pro проверена на реальном устройстве: чтение, DPI, подсветка и ограниченное переназначение кнопок через CLI с возвратом из дампа. GUI отложен до M8. План — в [`docs/ROADMAP.md`](docs/ROADMAP.md), устройство кода — в [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md).

Контейнерное окружение: [запуск `chimerazer-dev`](docs/DEVELOPMENT.md).

```bash
./.devcontainer/dev up --build -d
```

## Назначения кнопок Naga Pro

Обычный `set`/`restore` работает только при активном профиле 1 и в
[подтверждённой области](docs/protocol/naga-pro.md#обычная-запись-кнопок-после-m4).
Пример, меняющий боковую кнопку в профиле 1 через ресивер:

```bash
mkdir -p .artifacts
chimerazer dump --pid 0090 --out .artifacts/baseline.txt
chimerazer set button:1:5:key:q --pid 0090
chimerazer restore .artifacts/baseline.txt --pid 0090
```

`hypershift` — именованный включатель второго слоя; проверенная пара — средняя
кнопка `button:1:3:hypershift` и правая `button-hs:1:2:key:f8` через ресивер.
Прямой живой слой, банки 2–5 и слоты прокрутки 9/10 для обычной записи закрыты.
Дамп содержит только известное читаемое состояние и не является полным бэкапом.

## Сборка и проверки

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Доступ к устройству без root

```bash
sudo cp packaging/udev/70-chimerazer.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules && sudo udevadm trigger
```

## Лицензия

Apache-2.0. Атрибуция заимствованных данных — в [`NOTICE`](NOTICE).
