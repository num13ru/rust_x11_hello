FROM rust:1.86-bookworm

ARG RUST_TARGET=armv7-unknown-linux-musleabihf

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        gcc-arm-linux-gnueabihf \
        binutils-arm-linux-gnueabihf \
        libc6-dev-armhf-cross \
        file \
    && rustup target add "${RUST_TARGET}" \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /work

CMD ["bash", "scripts/build-kindle-armv7hf.sh"]
