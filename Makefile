-include .env

CONTAINER_CLI ?= docker
CONTAINER_RUN_ARGS ?= --cpus 8 --memory 8g
CONTAINER_BUILD_ARGS ?=
CONTAINER_PROXY ?=
APPLE_CONTAINER_NETWORK ?= default
PROXY ?= false
IMAGE := rust-kindle-armv7hf-builder
WORKDIR := /work

CONTAINER_PROXY_BUILD_ARGS :=
CONTAINER_PROXY_RUN_ARGS :=
CONTAINER_PROXY_WRAPPER :=
PROXY_ENV_NAMES := http_proxy https_proxy

ifneq ($(PROXY),true)
ifneq ($(PROXY),false)
$(error PROXY must be true or false)
endif
endif

ifeq ($(CONTAINER_CLI),container)
ifeq ($(PROXY),true)
ifeq ($(strip $(CONTAINER_PROXY)),)
CONTAINER_PROXY_WRAPPER := python3 scripts/apple-container-proxy.py \
	--network $(APPLE_CONTAINER_NETWORK) --
else
CONTAINER_PROXY_BUILD_ARGS := $(foreach name,$(PROXY_ENV_NAMES),\
	--build-arg $(name)=$(CONTAINER_PROXY))
CONTAINER_PROXY_RUN_ARGS := $(foreach name,$(PROXY_ENV_NAMES),\
	-e $(name)=$(CONTAINER_PROXY))
endif
endif
endif

.PHONY: check

check:
	cargo fmt --all --check
	cargo check --workspace
	cargo clippy --workspace --all-targets -- -D warnings
	cargo test --workspace
	sh -n kindle-extension/rust_x11_hello/bin/run.sh
	sh -n kindle-extension/rust_x11_hello/bin/probe-fb.sh
	bash -n scripts/build-kindle-armv7hf.sh
	bash -n scripts/deploy-kindle-mtp.sh
	python3 scripts/apple-container-proxy.py --help >/dev/null
	bash scripts/test-deploy-kindle-mtp.sh
	jq -e '.items == [{"name":"Run Paperpad (5m)","action":"PAPERPAD_WATCHDOG_SECONDS=300 sh bin/run.sh"},{"name":"Run Paperpad MXCFB (5m, experimental)","action":"PAPERPAD_WATCHDOG_SECONDS=300 PAPERPAD_DISPLAY_BACKEND=mxcfb sh bin/run.sh"},{"name":"Benchmark MXCFB GC16 (12)","action":"PAPERPAD_WATCHDOG_SECONDS=60 PAPERPAD_MXCFB_BENCHMARK_WAVEFORM=gc16 PAPERPAD_MXCFB_BENCHMARK_FRAMES=12 sh bin/run.sh --benchmark-mxcfb"},{"name":"Benchmark MXCFB DU (12, experimental)","action":"PAPERPAD_WATCHDOG_SECONDS=60 PAPERPAD_MXCFB_BENCHMARK_WAVEFORM=du PAPERPAD_MXCFB_BENCHMARK_FRAMES=12 sh bin/run.sh --benchmark-mxcfb"},{"name":"Benchmark MXCFB A2 (12, experimental)","action":"PAPERPAD_WATCHDOG_SECONDS=60 PAPERPAD_MXCFB_BENCHMARK_WAVEFORM=a2 PAPERPAD_MXCFB_BENCHMARK_FRAMES=12 sh bin/run.sh --benchmark-mxcfb"},{"name":"Benchmark A2 512px (12, experimental)","action":"PAPERPAD_WATCHDOG_SECONDS=60 PAPERPAD_MXCFB_BENCHMARK_WAVEFORM=a2 PAPERPAD_MXCFB_BENCHMARK_REGION=512x512 PAPERPAD_MXCFB_BENCHMARK_FRAMES=12 sh bin/run.sh --benchmark-mxcfb"},{"name":"Benchmark A2 256px (12, experimental)","action":"PAPERPAD_WATCHDOG_SECONDS=60 PAPERPAD_MXCFB_BENCHMARK_WAVEFORM=a2 PAPERPAD_MXCFB_BENCHMARK_REGION=256x256 PAPERPAD_MXCFB_BENCHMARK_FRAMES=12 sh bin/run.sh --benchmark-mxcfb"},{"name":"Benchmark A2 64px (12, experimental)","action":"PAPERPAD_WATCHDOG_SECONDS=60 PAPERPAD_MXCFB_BENCHMARK_WAVEFORM=a2 PAPERPAD_MXCFB_BENCHMARK_REGION=64x64 PAPERPAD_MXCFB_BENCHMARK_FRAMES=12 sh bin/run.sh --benchmark-mxcfb"},{"name":"Benchmark MXCFB DU4 (12, experimental)","action":"PAPERPAD_WATCHDOG_SECONDS=60 PAPERPAD_MXCFB_BENCHMARK_WAVEFORM=du4 PAPERPAD_MXCFB_BENCHMARK_FRAMES=12 sh bin/run.sh --benchmark-mxcfb"},{"name":"Inspect framebuffer metadata","action":"sh bin/probe-fb.sh"}]' \
		kindle-extension/rust_x11_hello/menu.json >/dev/null

.PHONY: image _image build _build verify shell _shell clean clean-gnu clean-target

image:
	$(CONTAINER_PROXY_WRAPPER) $(MAKE) _image

build:
	$(CONTAINER_PROXY_WRAPPER) $(MAKE) _build

shell:
	$(CONTAINER_PROXY_WRAPPER) $(MAKE) _shell

_image:
	$(CONTAINER_CLI) build $(CONTAINER_BUILD_ARGS) $(CONTAINER_PROXY_BUILD_ARGS) -t $(IMAGE) .

_build: _image
	$(CONTAINER_CLI) run --rm $(CONTAINER_RUN_ARGS) $(CONTAINER_PROXY_RUN_ARGS) \
		-v "$$(pwd)":$(WORKDIR) \
		-w $(WORKDIR) \
		$(IMAGE)

verify:
	$(CONTAINER_CLI) run --rm $(CONTAINER_RUN_ARGS) $(CONTAINER_PROXY_RUN_ARGS) \
		-v "$$(pwd)":$(WORKDIR) \
		-w $(WORKDIR) \
		$(IMAGE) \
		bash scripts/check-static.sh

_shell: _image
	$(CONTAINER_CLI) run --rm -it $(CONTAINER_RUN_ARGS) $(CONTAINER_PROXY_RUN_ARGS) \
		-v "$$(pwd)":$(WORKDIR) \
		-w $(WORKDIR) \
		$(IMAGE) \
		bash

clean-gnu:
	rm -rf target/armv7-unknown-linux-gnueabihf

clean-target:
	rm -rf target

clean:
	rm -rf target logs
	rm -f kindle-extension/rust_x11_hello/bin/rust_x11_hello
