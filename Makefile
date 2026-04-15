.PHONY: setup build release install uninstall clean

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
