IMAGE := rust-kindle-armv7hf-builder
WORKDIR := /work

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
	rm -f kindle-extension/rust_hello/bin/rust_hello
