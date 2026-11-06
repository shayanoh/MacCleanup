release:
	cargo build --release

install: release
	cp target/release/maccleanup ~/.local/bin

.PHONY: install release
