release:
	cargo build --release

install:
	cargo install --path .

.PHONY: install release
