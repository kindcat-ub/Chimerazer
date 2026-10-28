# Chimerazer

Неофициальный open-source конфигуратор периферии Razer для Linux. Настраивает устройство, записывает параметры в его память и не держит фоновых процессов.

Статус: **M0 — фундамент**. С реальными устройствами программа пока не работает. План — в [`docs/ROADMAP.md`](docs/ROADMAP.md), устройство кода — в [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md).

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
