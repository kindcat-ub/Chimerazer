# Реестр источников

На эти ID ссылаются таблицы команд (`docs/protocol/<устройство>.md`) и код (`CommandSpec::sources`). Тест проверяет, что каждый используемый ID здесь определён.

Зафиксированные коммиты:

| Проект | Лицензия | Коммит | Что берём |
|---|---|---|---|
| [OpenSnek](https://github.com/gh123man/OpenSnek) | Apache-2.0 | `b4e0f666d21b52efc0a64a0e128a1278aa1b3ad2` | факты, данные захватов, логику (с атрибуцией в `NOTICE`) |
| [OpenRazer](https://github.com/openrazer/openrazer) | GPL-2.0-or-later | `477f4d6d3ab7e9deb6310d5bd3742560c5cbe80d` | только факты |
| [Snakecharmer](https://github.com/asavs/snakecharmer) | GPL-2.0-or-later | `f3155720095f7b78fa8fd661268993143f3e3664` | только факты и подходы |

## Источники

| ID | Проект | Путь | Содержание |
|---|---|---|---|
| `opensnek-usb-protocol` | OpenSnek | `docs/protocol/USB_PROTOCOL.md` | Формат репорта, общие команды Razer, коды статусов. |
| `opensnek-crud-spec` | OpenSnek | `docs/protocol/USB_PROFILE_CRUD_SPEC.md` | Модель хранилищ профилей, метаданные профиля, чтение состояния подсветки. Проверено на Basilisk V3 Pro, не на Naga Pro. |
| `opensnek-device-support` | OpenSnek | `docs/DEVICE_SUPPORT.md` (раздел Naga Pro), `OpenSnek/Sources/OpenSnekCore/DeviceSupport.swift` | Матрица поддержки Naga Pro, проверенная контрибьюторами. |
| `opensnek-naga-default-banks` | OpenSnek | `captures/usb/2026-10-05-naga-pro-native-default-banks/` | Захват на Naga Pro через свисток `1532:0090`: профили, DPI, яркость, блоки кнопок. |
| `opensnek-naga-button-encoding` | OpenSnek | `captures/usb/2026-10-05-naga-pro-button-encoding-hid/`, `OpenSnek/Tests/OpenSnekTests/USBButtonHydrationTests.swift` | Раскладка ответа `02:8C` на Naga Pro, блоки кнопок, записанные Synapse. |
| `opensnek-naga-side-panels` | OpenSnek | `captures/usb/2026-10-06-naga-pro-side-panel-validation/` | Инвентарь `02:84` (30 слотов), проверка нажатием всех трёх боковых панелей. |
| `opensnek-naga-v3-pro-effects` | OpenSnek | `captures/usb/2026-10-17-naga-v3-pro/README.md` (раздел Lighting) | Эффекты `0F:02` на Naga V3 Pro: ответ `0F:82` с ожидаемым effect id, возврат Spectrum формой `01 <led> 03 00 00 00`. Не Naga Pro. |
| `openrazer-common` | OpenRazer | `driver/razercommon.h`, `driver/razercommon.c` | Структура репорта, CRC, статусы, параметры SET_REPORT/GET_REPORT. |
| `openrazer-mouse` | OpenRazer | `driver/razermouse_driver.c`, `driver/razermouse_driver.h`, `driver/razerchromacommon.c` | Возможности Naga Pro, PID, интерфейс, transaction id, задержки ответа. |
| `snakecharmer-spec` | Snakecharmer | `docs/SPEC.md`, `docs/ONBOARD-KEYMAP.md` | Расхождения VARSTORE/NOSTORE при записи DPI, подход safety-latch. |
| `chimerazer-m1-reads` | Chimerazer | `fixtures/naga-pro/our-hardware-frames.txt` | Сырые кадры чтения с нашего экземпляра Naga Pro, провод `008F` и свисток `0090` (M1, 2026-10-28). |
| `chimerazer-m2-validation` | Chimerazer | `docs/validation/naga-pro/` | Протоколы первичной аппаратной валидации записи на нашем экземпляре Naga Pro. |
