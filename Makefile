IMAGE := rust-kindle-armv7hf-builder
WORKDIR := /work

.PHONY: check check-protocol check-companion

check: check-protocol check-companion
	cargo fmt --check
	cargo check
	cargo clippy --all-targets -- -D warnings
	cargo test
	sh -n kindle-extension/rust_x11_hello/bin/run.sh
	sh -n kindle-extension/rust_x11_hello/bin/show.sh
	sh -n kindle-extension/rust_x11_hello/bin/stop.sh
	bash -n scripts/deploy-kindle-mtp.sh
	bash scripts/test-deploy-kindle-mtp.sh
	jq empty kindle-extension/rust_x11_hello/menu.json

check-protocol:
	cargo fmt --manifest-path crates/transport-protocol/Cargo.toml -- --check
	cargo check --manifest-path crates/transport-protocol/Cargo.toml
	cargo clippy --manifest-path crates/transport-protocol/Cargo.toml --all-targets -- -D warnings
	cargo test --manifest-path crates/transport-protocol/Cargo.toml

check-companion:
	cargo fmt --manifest-path tools/companion/Cargo.toml -- --check
	cargo check --manifest-path tools/companion/Cargo.toml
	cargo clippy --manifest-path tools/companion/Cargo.toml --all-targets -- -D warnings
	cargo test --manifest-path tools/companion/Cargo.toml --all-targets

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
