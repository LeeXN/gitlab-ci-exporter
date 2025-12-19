BINARY := gitlab-ci-exporter
PORT ?= 3000

.PHONY: all build run test fmt clippy clean docker-build docker-run

all: build

build:
	cargo build --release

run:
	cargo run --release

test:
	cargo test

fmt:
	cargo fmt --all -- --check

clippy:
	cargo clippy --all -- -D warnings

clean:
	cargo clean

docker-build:
	docker build -t leex2019/$(BINARY):latest .

docker-run:
	docker run --rm -p $(PORT):$(PORT) -v $(PWD)/config.toml:/app/config.toml leex2019/$(BINARY):latest