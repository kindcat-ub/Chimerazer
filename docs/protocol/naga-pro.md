# Razer Naga Pro — таблица команд

Устройство: Razer Naga Pro (RZ01-0342).

| PID | Подключение | Интерфейс | Transaction ID | Задержка ответа (OpenRazer) |
|---|---|---|---|---|
| `1532:008F` | провод | 0 (usage `0x01:0x02`, feature 90 байт) | `0x1F` | 600 мкс |
| `1532:0090` | свисток 2.4 ГГц | 0 | `0x1F` | 31 мс |

Источники: `openrazer-mouse`, `opensnek-device-support`. Задержку выбирает транспорт в M1. OpenSnek вместо фиксированной паузы опрашивает ответ 6 раз с шагом 30–35 мс.

Статусы описаны в `docs/ARCHITECTURE.md`. ID источников — в `docs/protocol/SOURCES.md`.

## Команды чтения

Эта таблица и `crates/devices/src/naga_pro.rs` (`COMMANDS`) должны совпадать. Это проверяет тест `crates/devices/tests/catalog_docs.rs`.

| Команда | Назначение | Size | Аргументы запроса | Статус | Источники | Примечание |
|---|---|---|---|---|---|---|
| `00:81` | версия прошивки | `02` | — | `OurHardware` | `chimerazer-m1-reads`, `opensnek-usb-protocol`, `openrazer-common` | Отвечает на txn `0x1F` (OpenSnek указывает `0xFF`). На нашем экземпляре: `02 03`. Строка `0x00112100` в захватах — это locationID macOS, а не прошивка. |
| `00:82` | серийный номер | `16` | — | `OurHardware` | `chimerazer-m1-reads`, `opensnek-usb-protocol`, `openrazer-common` | Отвечает на txn `0x1F` (OpenSnek указывает `0xFF`). |
| `00:84` | режим устройства | `02` | — | `OurHardware` | `chimerazer-m1-reads`, `opensnek-usb-protocol`, `openrazer-common` | `00 00` — аппаратный режим, `03 00` — режим драйвера. |
| `00:85` | частота опроса | `01` | — | `OurHardware` | `chimerazer-m1-reads`, `opensnek-usb-protocol`, `openrazer-mouse` | `01` = 1000 Гц, `02` = 500 Гц, `08` = 125 Гц. |
| `00:87` | сводка профилей | `00` | — | `OurHardware` | `chimerazer-m1-reads`, `opensnek-naga-default-banks` | На Naga Pro вернула `02 02 00`. Неавторитетна, активный профиль берём из `05:84`. |
| `02:84` | инвентарь кнопок | `05` | `00 00 00 00 00` | `OurHardware` | `chimerazer-m1-reads`, `opensnek-naga-side-panels` | `[count, slot…]`; на Naga Pro 30 слотов. |
| `02:8C` | функция кнопки | `0A` | `[storage, slot, hypershift]` | `OurHardware` | `chimerazer-m1-reads`, `opensnek-naga-default-banks`, `opensnek-naga-button-encoding` | Ответ: `args[2]` — сохранённый флаг слоя (не эхо), блок всегда в `args[3..10]`. |
| `04:85` | DPI | `07` | `[storage]` | `OurHardware` | `chimerazer-m1-reads`, `opensnek-naga-default-banks`, `openrazer-mouse` | `[storage, x_be16, y_be16]`. |
| `04:86` | стадии DPI | `26` | `[storage]` | `OurHardware` | `chimerazer-m1-reads`, `opensnek-naga-default-banks`, `openrazer-mouse` | `[storage, active_id, count, count×[id, x, y, 0, 0]]`; `active_id` — токен, а не индекс. В хранимых банках профилей два «резервных» байта строки не нулевые: в захвате OpenSnek они повторяют значение DPI (`01 01 90 01 90 01 90`); в живом слое (`storage 0`) на нашем экземпляре они нулевые. При записи их нужно сохранять как прочитано. |
| `05:80` | число профилей | `00` | — | `OurHardware` | `chimerazer-m1-reads`, `opensnek-naga-default-banks` | |
| `05:81` | список профилей | `00` | — | `OurHardware` | `chimerazer-m1-reads`, `opensnek-naga-default-banks` | `[max_id, assigned…]`. |
| `05:84` | активный профиль | `00` | — | `OurHardware` | `chimerazer-m1-reads`, `opensnek-naga-default-banks` | Может вернуть `data_size = 0` с валидным `args[0]`. |
| `05:88` | метаданные профиля (кусок) | `50` | `[profile, off_hi, off_lo, 00, fa]` | `SourceOnly` | `opensnek-crud-spec`, `opensnek-naga-default-banks` | Для неназначенных банков на Naga Pro чтение не проходит. |
| `07:80` | уровень батареи | `02` | — | `OurHardware` | `chimerazer-m1-reads`, `opensnek-usb-protocol`, `openrazer-mouse` | `args[1]` в шкале 0–255. Других показателей батареи (напряжение, ток, ёмкость) в OpenSnek, OpenRazer и Snakecharmer нет; неизвестные команды не перебираем. |
| `07:81` | порог низкого заряда | `01` | — | `OurHardware` | `chimerazer-m1-reads`, `opensnek-usb-protocol`, `openrazer-mouse` | Сырое значение `0x0C..0x3F`. |
| `07:83` | время до сна | `02` | — | `OurHardware` | `chimerazer-m1-reads`, `opensnek-usb-protocol`, `openrazer-mouse` | Секунды big-endian, 60..900. |
| `07:84` | статус зарядки | `02` | — | `OurHardware` | `chimerazer-m1-reads`, `opensnek-usb-protocol`, `openrazer-mouse` | `args[1] == 1` — заряжается. |
| `0F:82` | состояние эффекта LED | `0C` | `[storage, led]` | `OurHardware` | `chimerazer-m1-reads`, `opensnek-crud-spec` | Ответ: `[storage, led, effect, …]`, в дампе — `args[2..12]`. `effect`: `01` статический (`01 00 00 01 r g b`), `03` Spectrum (по OpenSnek). Эхо `storage` в профилях ненадёжно. |
| `0F:84` | яркость LED | `03` | `[storage, led]` | `OurHardware` | `chimerazer-m1-reads`, `opensnek-naga-default-banks`, `opensnek-device-support` | LED: `01` колесо, `04` логотип. |

`data_size` в ответах Naga Pro ненадёжен: `05:80` и `05:84` возвращают `0` при значимом `args[0]`, `02:84` — `5` при 30 слотах в аргументах. Декодеры его не используют.

Хранилища (`storage`): `0` — живой слой, `1..5` — onboard-профили (`opensnek-crud-spec`).

## Команды записи

В коде (`WRITE_COMMANDS`): `00:05`, `04:05`, `04:06`, `0F:04`, `0F:02` (две формы, различаются размером); их строки сверяются тестом так же, как чтения. Остальные — справочно. В обычном режиме (`restore`) писать можно только командами `OurHardware` и только в подтверждённых сочетаниях «подключение + storage» (`VERIFIED_WRITES` в коде):

| Команда | Провод `008F` | Свисток `0090` |
|---|---|---|
| `04:05` DPI | `storage 0`, `storage 1` | `storage 0`, `storage 1` |
| `04:06` стадии DPI | `storage 0`, `storage 1` | `storage 1` |
| `0F:04` яркость (LED `01`, `04`) | `storage 0`, `storage 1` | `storage 1` |

Запись в `storage 1` сохраняется после переподключения и после полного выключения мыши. Профили 2–5 и живой слой через свисток для стадий и яркости не допускаются до отдельной проверки. Первая запись в новом сочетании — только через `chimerazer validate` (процедура в `docs/ARCHITECTURE.md`).

| Команда | Назначение | Size | Аргументы | Статус | Источники | Примечание |
|---|---|---|---|---|---|---|
| `00:05` | частота опроса | `01` | `[rate]` | `SourceOnly` | `opensnek-usb-protocol`, `openrazer-mouse` | |
| `02:0C` | функция кнопки | `0A` | `[storage, slot, hypershift, block×7]` | `ExternalHardware` | `opensnek-naga-side-panels`, `opensnek-naga-default-banks` | Устройство иногда возвращает ошибку, хотя запись прошла. Проверять только чтением. |
| `04:05` | DPI | `07` | `[storage, x_be16, y_be16, 0, 0]` | `OurHardware` | `chimerazer-m2-validation`, `opensnek-device-support`, `openrazer-mouse`, `snakecharmer-spec` | Наш экземпляр: запись в `storage 0` применяется сразу и сбрасывается при переподключении (`docs/validation/naga-pro/04-05-dpi.md`). Источники расходятся по флагу сохранения: OpenRazer всегда шлёт `01`, Snakecharmer — `00`, OpenSnek — `00` для живого слоя. |
| `04:06` | стадии DPI | `26` | `[storage, active_id, count, rows]` | `OurHardware` | `chimerazer-m2-validation`, `opensnek-device-support`, `openrazer-mouse` | OpenSnek всегда пишет 5 строк с `count = 5`. |
| `05:02` | создать профиль | `01` | `[profile]` | `SourceOnly` | `opensnek-crud-spec` | Сбивает DPI и яркость, после него нужна перезапись. |
| `05:03` | удалить профиль | `01` | `[profile]` | `SourceOnly` | `opensnek-crud-spec` | Только убирает профиль из цикла, данные не стирает. |
| `05:04` | выбрать профиль | `01` | `[profile]` | `SourceOnly` | `opensnek-crud-spec` | Неназначенный профиль отклоняется. |
| `05:08` | метаданные профиля (кусок) | `50` | `[profile, off_hi, off_lo, 00, fa, data×75]` | `SourceOnly` | `opensnek-crud-spec` | Писать только все 4 куска. Последний может не подтвердиться. |
| `07:01` | порог низкого заряда | `01` | `[raw]` | `SourceOnly` | `opensnek-usb-protocol`, `openrazer-mouse` | |
| `07:03` | время до сна | `02` | `[secs_be16]` | `SourceOnly` | `opensnek-usb-protocol`, `openrazer-mouse` | |
| `0F:02` | эффект LED: статический цвет | `09` | `[storage, led, 01, 00, 00, 01, r, g, b]` | `ExternalHardware` | `opensnek-device-support` | В коде: `SET_LED_STATIC`. Чтение обратно — `0F:82`. На нашем экземпляре работает в живом слое и в профиле 1, переживает выключение (`docs/validation/naga-pro/0F-02-led-effect.md`). |
| `0F:02` | эффект LED: Spectrum | `06` | `[storage, led, 03, 00, 00, 00]` | `SourceOnly` | `opensnek-naga-v3-pro-effects` | В коде: `SET_LED_SPECTRUM`. Этой формой OpenSnek возвращал Spectrum на Naga V3 Pro. На нашем экземпляре возвращает Spectrum байт в байт (`docs/validation/naga-pro/0F-02-led-effect.md`). |
| `0F:04` | яркость LED | `03` | `[storage, led, value]` | `OurHardware` | `chimerazer-m2-validation`, `opensnek-device-support` | |

## Проверено на нашем экземпляре (M1)

- Заголовок, CRC и эхо transaction id `0x1F` корректны на настоящих ответах по проводу и через свисток (`fixtures/naga-pro/our-hardware-frames.txt`).
- Ответы по проводу и через свисток совпадают байт в байт, кроме батареи (`07:80`) и зарядки (`07:84`), которые зависят от кабеля.
- `00:81` / `00:82` отвечают на txn `0x1F`.
- `05:88` (метаданные профиля) возвращает `03` (Failed) даже для назначенного профиля 1 — формат запроса для Naga Pro не подтверждён.
- Пока кабель подключён, свисток `0090` отвечает статусом `04` (Timeout) на любую команду: мышь работает по проводу, за свистком её нет.

## Открытые вопросы для M1–M2

- Подсветка 12-кнопочной панели (LED `0x05`) известна только из OpenRazer и в таблицу не внесена.
