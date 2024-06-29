generate:
	cd flatpak && poetry install && poetry run python ./flatpak-cargo-generator.py ../Cargo.lock -o ../cargo-sources.json

install:
	flatpak run org.flatpak.Builder --user --install --force-clean build-dir io.github.mawi1.oondl.json

run:
	RUST_LOG=debug cargo run

.PHONY: generate install run