# Окружение разработки

Постоянное окружение проекта — один контейнер `chimerazer-dev`. Требуются
Linux, Docker Engine с Compose v2+ и доступ пользователя к Docker.
Запускай команды от обычного пользователя, не через sudo.

Из корня checkout/worktree:

```bash
./.devcontainer/dev up --build -d
./.devcontainer/dev exec dev bash
```

Скрипт передаёт Compose UID/GID пользователя и создаёт локальный каталог
`.artifacts/` до монтирования volumes, чтобы Docker не создал его от root.
Все аргументы скрипта передаются штатному Docker Compose.

## Инструменты и проверки

Dockerfile содержит Rust 1.89.0 (MSRV workspace), Cargo, Clippy, rustfmt,
компилятор и линкер, pkg-config, Git, CA-сертификаты, GDB, strace, usbutils,
udev и ripgrep. Новые зависимости добавляются в Dockerfile; ручная установка
в работающий контейнер не является частью окружения.

```bash
./.devcontainer/dev exec -T dev cargo fmt --all --check
./.devcontainer/dev exec -T dev cargo clippy --workspace --all-targets -- -D warnings
./.devcontainer/dev exec -T dev cargo test --workspace
```

CI отдельно собирает это окружение и запускает те же проверки. Существующие
проверки stable и MSRV остаются самостоятельной проверкой проекта.

## Исходники, кеш и результаты

- Checkout подключён через bind mount в `/workspace`; изменения исходников
  сразу видны на хосте и переживают удаление контейнера.
- Cargo cache — named volume `chimerazer-dev_cargo-cache`, путь
  `/home/dev/.cargo`.
- Сборка — named volume `chimerazer-dev_build-cache`, путь
  `/workspace/.artifacts/target`. На хосте этот mount не показывает содержимое
  volume; бинарники запускаются или копируются из контейнера.
- Временные файлы (`TMPDIR`) — `/workspace/.artifacts/tmp`; логи, дампы и
  локальные результаты также сохраняются внутри `.artifacts/`.
- `.artifacts/` исключён из Git. Фикстуры добавляются только после очистки
  идентификаторов. В образ исходники и локальные файлы не копируются.

```bash
./.devcontainer/dev stop
./.devcontainer/dev up -d
./.devcontainer/dev down
./.devcontainer/dev up --build -d
```

`down` удаляет контейнер, сохраняя исходники и оба volumes. `down -v` удаляет
кеш и сборку. Исходники и обычные файлы `.artifacts/` остаются на хосте.
При смене UID/GID владельца существующих volumes потребуется согласовать
с новым пользователем; штатный запуск их права не переписывает.

## Границы

Контейнер работает от UID/GID разработчика, без capabilities, privileged,
Docker socket, host network и проброса устройств. Диагностические утилиты
установлены, но доступ к USB/hidraw и разрешения для аппаратной отладки
настраиваются отдельной задачей с минимально необходимыми правами.
По умолчанию аппаратные проверки из контейнера недоступны.

Git-операции выполняются на хосте: linked worktree может ссылаться на Git
metadata вне bind mount, а ключи и учётные данные в контейнер не передаются.
Фиксированное имя позволяет держать одно окружение проекта; перед запуском
из другого worktree выполни `down` из текущего. Не подключай одновременно
разные worktree к одному кешу сборки.

Окружение обслуживает разработку Chimerazer. Новые сервисы, фоновые демоны
и отдельная инфраструктура добавляются только при доказанной необходимости.
