.PHONY: setup build release install uninstall clean start-simulator-ios start-simulator-android

# Overridable: make start-simulator-ios DEVICE="iPhone 16 Pro"
DEVICE ?= iPhone
# Overridable: make start-simulator-android AVD=Pixel_7_API_34
AVD ?=

setup:
	@if ! command -v cargo >/dev/null 2>&1; then \
		echo "Installing Rust via rustup..."; \
		curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y; \
		. "$$HOME/.cargo/env"; \
	else \
		echo "Rust already installed: $$(cargo --version)"; \
	fi
	cargo fetch

build:
	cargo build

release:
	cargo build --release

install:
	cargo install --path . --force

uninstall:
	cargo uninstall mdev

clean:
	cargo clean

start-simulator-ios:
	cargo run --quiet -- simulator ios --device "$(DEVICE)"

start-simulator-android:
	cargo run --quiet -- simulator android $(if $(AVD),--avd "$(AVD)")
