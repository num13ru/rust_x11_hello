FROM rust:1.86-bookworm

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        gcc-arm-linux-gnueabihf \
        binutils-arm-linux-gnueabihf \
        libc6-dev-armhf-cross \
        file \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /work

CMD ["bash", "scripts/build-kindle-armv7hf-musl.sh"]
