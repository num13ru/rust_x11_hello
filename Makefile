IMAGE := rust-kindle-armv7hf-builder
WORKDIR := /work

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
	bash scripts/test-deploy-kindle-mtp.sh
	jq -e '.items == [{"name":"Run Paperpad (5m)","action":"PAPERPAD_WATCHDOG_SECONDS=300 sh bin/run.sh"},{"name":"Run Paperpad MXCFB (5m, experimental)","action":"PAPERPAD_WATCHDOG_SECONDS=300 PAPERPAD_DISPLAY_BACKEND=mxcfb sh bin/run.sh"},{"name":"Benchmark MXCFB GC16 (12)","action":"PAPERPAD_WATCHDOG_SECONDS=60 PAPERPAD_MXCFB_BENCHMARK_WAVEFORM=gc16 PAPERPAD_MXCFB_BENCHMARK_FRAMES=12 sh bin/run.sh --benchmark-mxcfb"},{"name":"Benchmark MXCFB DU (12, experimental)","action":"PAPERPAD_WATCHDOG_SECONDS=60 PAPERPAD_MXCFB_BENCHMARK_WAVEFORM=du PAPERPAD_MXCFB_BENCHMARK_FRAMES=12 sh bin/run.sh --benchmark-mxcfb"},{"name":"Benchmark MXCFB A2 (12, experimental)","action":"PAPERPAD_WATCHDOG_SECONDS=60 PAPERPAD_MXCFB_BENCHMARK_WAVEFORM=a2 PAPERPAD_MXCFB_BENCHMARK_FRAMES=12 sh bin/run.sh --benchmark-mxcfb"},{"name":"Benchmark MXCFB DU4 (12, experimental)","action":"PAPERPAD_WATCHDOG_SECONDS=60 PAPERPAD_MXCFB_BENCHMARK_WAVEFORM=du4 PAPERPAD_MXCFB_BENCHMARK_FRAMES=12 sh bin/run.sh --benchmark-mxcfb"},{"name":"Inspect framebuffer metadata","action":"sh bin/probe-fb.sh"}]' \
		kindle-extension/rust_x11_hello/menu.json >/dev/null

.PHONY: image build verify shell clean clean-gnu clean-target

image:
	docker build -t $(IMAGE) .

build: image
	docker run --rm \
		-v "$$(pwd)":$(WORKDIR) \
		-w $(WORKDIR) \
		$(IMAGE)

verify:
	docker run --rm \
		-v "$$(pwd)":$(WORKDIR) \
		-w $(WORKDIR) \
		$(IMAGE) \
		bash scripts/check-static.sh

shell: image
	docker run --rm -it \
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
