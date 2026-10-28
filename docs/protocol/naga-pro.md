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
| `00:81` | версия прошивки | `02` | — | `SourceOnly` | `opensnek-usb-protocol`, `openrazer-common` | В OpenSnek для этой команды указан txn `0xFF`. Строка `0x00112100` в захватах — это locationID macOS, а не прошивка. |
| `00:82` | серийный номер | `16` | — | `SourceOnly` | `opensnek-usb-protocol`, `openrazer-common` | В OpenSnek для этой команды указан txn `0xFF`. |
| `00:84` | режим устройства | `02` | — | `SourceOnly` | `opensnek-usb-protocol`, `openrazer-common` | `00 00` — аппаратный режим, `03 00` — режим драйвера. |
| `00:85` | частота опроса | `01` | — | `SourceOnly` | `opensnek-usb-protocol`, `openrazer-mouse` | `01` = 1000 Гц, `02` = 500 Гц, `08` = 125 Гц. |
| `00:87` | сводка профилей | `00` | — | `ExternalHardware` | `opensnek-naga-default-banks` | На Naga Pro вернула `02 02 00`. Неавторитетна, активный профиль берём из `05:84`. |
| `02:84` | инвентарь кнопок | `05` | `00 00 00 00 00` | `ExternalHardware` | `opensnek-naga-side-panels` | `[count, slot…]`; на Naga Pro 30 слотов. |
| `02:8C` | функция кнопки | `0A` | `[storage, slot, hypershift]` | `ExternalHardware` | `opensnek-naga-default-banks`, `opensnek-naga-button-encoding` | Ответ: `args[2]` — сохранённый флаг слоя (не эхо), блок всегда в `args[3..10]`. |
| `04:85` | DPI | `07` | `[storage]` | `ExternalHardware` | `opensnek-naga-default-banks`, `openrazer-mouse` | `[storage, x_be16, y_be16]`. |
| `04:86` | стадии DPI | `26` | `[storage]` | `ExternalHardware` | `opensnek-naga-default-banks`, `openrazer-mouse` | `[storage, active_id, count, count×[id, x, y, 0, 0]]`; `active_id` — токен, а не индекс. На Naga Pro два «резервных» байта строки не нулевые: в захвате они повторяют значение DPI (`01 01 90 01 90 01 90`). При записи их нужно сохранять. |
| `05:80` | число профилей | `00` | — | `ExternalHardware` | `opensnek-naga-default-banks` | |
| `05:81` | список профилей | `00` | — | `ExternalHardware` | `opensnek-naga-default-banks` | `[max_id, assigned…]`. |
| `05:84` | активный профиль | `00` | — | `ExternalHardware` | `opensnek-naga-default-banks` | Может вернуть `data_size = 0` с валидным `args[0]`. |
| `05:88` | метаданные профиля (кусок) | `50` | `[profile, off_hi, off_lo, 00, fa]` | `SourceOnly` | `opensnek-crud-spec`, `opensnek-naga-default-banks` | Для неназначенных банков на Naga Pro чтение не проходит. |
| `07:80` | уровень батареи | `02` | — | `SourceOnly` | `opensnek-usb-protocol`, `openrazer-mouse` | `args[1]` в шкале 0–255. |
| `07:81` | порог низкого заряда | `01` | — | `SourceOnly` | `opensnek-usb-protocol`, `openrazer-mouse` | Сырое значение `0x0C..0x3F`. |
| `07:83` | время до сна | `02` | — | `SourceOnly` | `opensnek-usb-protocol`, `openrazer-mouse` | Секунды big-endian, 60..900. |
| `07:84` | статус зарядки | `02` | — | `SourceOnly` | `opensnek-usb-protocol`, `openrazer-mouse` | `args[1] == 1` — заряжается. |
| `0F:82` | состояние эффекта LED | `0C` | `[storage, led]` | `SourceOnly` | `opensnek-crud-spec` | Проверено только на Basilisk V3 Pro. |
| `0F:84` | яркость LED | `03` | `[storage, led]` | `ExternalHardware` | `opensnek-naga-default-banks`, `opensnek-device-support` | LED: `01` колесо, `04` логотип. |

Хранилища (`storage`): `0` — живой слой, `1..5` — onboard-профили (`opensnek-crud-spec`).

## Команды записи (справочно, в коде их нет)

Писать в устройство до M2 запрещено. Писать можно будет только командами со статусом `OurHardware`.

| Команда | Назначение | Size | Аргументы | Статус | Источники | Примечание |
|---|---|---|---|---|---|---|
| `00:05` | частота опроса | `01` | `[rate]` | `SourceOnly` | `opensnek-usb-protocol`, `openrazer-mouse` | |
| `02:0C` | функция кнопки | `0A` | `[storage, slot, hypershift, block×7]` | `ExternalHardware` | `opensnek-naga-side-panels`, `opensnek-naga-default-banks` | Устройство иногда возвращает ошибку, хотя запись прошла. Проверять только чтением. |
| `04:05` | DPI | `07` | `[storage, x_be16, y_be16, 0, 0]` | `ExternalHardware` | `opensnek-device-support`, `openrazer-mouse`, `snakecharmer-spec` | Источники расходятся по флагу сохранения: OpenRazer всегда шлёт `01`, Snakecharmer — `00`, OpenSnek — `00` для живого слоя. |
| `04:06` | стадии DPI | `26` | `[storage, active_id, count, rows]` | `ExternalHardware` | `opensnek-device-support`, `openrazer-mouse` | OpenSnek всегда пишет 5 строк с `count = 5`. |
| `05:02` | создать профиль | `01` | `[profile]` | `SourceOnly` | `opensnek-crud-spec` | Сбивает DPI и яркость, после него нужна перезапись. |
| `05:03` | удалить профиль | `01` | `[profile]` | `SourceOnly` | `opensnek-crud-spec` | Только убирает профиль из цикла, данные не стирает. |
| `05:04` | выбрать профиль | `01` | `[profile]` | `SourceOnly` | `opensnek-crud-spec` | Неназначенный профиль отклоняется. |
| `05:08` | метаданные профиля (кусок) | `50` | `[profile, off_hi, off_lo, 00, fa, data×75]` | `SourceOnly` | `opensnek-crud-spec` | Писать только все 4 куска. Последний может не подтвердиться. |
| `07:01` | порог низкого заряда | `01` | `[raw]` | `SourceOnly` | `opensnek-usb-protocol`, `openrazer-mouse` | |
| `07:03` | время до сна | `02` | `[secs_be16]` | `SourceOnly` | `opensnek-usb-protocol`, `openrazer-mouse` | |
| `0F:02` | статический цвет LED | `09` | `[storage, led, 01, 00, 00, 01, r, g, b]` | `ExternalHardware` | `opensnek-device-support` | |
| `0F:04` | яркость LED | `03` | `[storage, led, value]` | `ExternalHardware` | `opensnek-device-support` | |

## Открытые вопросы для M1–M2

- Какой transaction id нужен для `00:81` / `00:82`: `0x1F` или `0xFF`.
- Что из записанного переживает переподключение (флаг хранения DPI).
- Совпадает ли состояние, прочитанное по проводу и через свисток. Все внешние захваты сделаны только через свисток.
- Подсветка 12-кнопочной панели (LED `0x05`) известна только из OpenRazer и в таблицу не внесена.
