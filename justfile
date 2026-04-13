default:
  @just --list

build:
  cargo build

build-release:
  cargo build --release

run:
  cargo run

json:
  cargo run -- --json

json-pretty:
  cargo run -- --json --pretty

tui:
  cargo run

test:
  cargo test

check:
  cargo check

clippy:
  cargo clippy -- -D warnings

fmt:
  cargo fmt

clean:
  cargo clean

install:
  cargo install --path .