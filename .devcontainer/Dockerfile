FROM rust:1.89.0-slim-bookworm@sha256:d7fc7de78bb8c1469933aeecbf801314d30d7d6e9f0578bba4cfa285bfa37fe6

ARG LOCAL_UID=1000
ARG LOCAL_GID=1000

RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential ca-certificates git pkg-config \
    gdb strace usbutils udev ripgrep \
    && rm -rf /var/lib/apt/lists/* \
    && rustup component add clippy rustfmt \
    && groupadd --gid "$LOCAL_GID" dev \
    && useradd --uid "$LOCAL_UID" --gid "$LOCAL_GID" --create-home dev \
    && mkdir -p /home/dev/.cargo /workspace/.artifacts/target \
    && chown -R dev:dev /home/dev/.cargo /workspace

ENV CARGO_HOME=/home/dev/.cargo \
    CARGO_TARGET_DIR=/workspace/.artifacts/target \
    TMPDIR=/workspace/.artifacts/tmp

USER dev
WORKDIR /workspace
CMD ["sleep", "infinity"]
