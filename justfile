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

# Screenshot the TUI in a headless tmux session after a fetch settles.
# Width/height default to a comfortable 200x60; override e.g. `just screenshot 160 50 grid.txt`.
screenshot width="200" height="60" out="/tmp/quotas.txt" wait="10":
  #!/usr/bin/env bash
  set -eu
  tmux kill-session -t quotas_snap 2>/dev/null || true
  tmux new-session -d -s quotas_snap -x {{width}} -y {{height}} './target/debug/quotas'
  sleep {{wait}}
  tmux capture-pane -t quotas_snap -p > {{out}}
  tmux kill-session -t quotas_snap
  echo "wrote {{out}}"

# Same but into the detail view (sends Enter after the wait).
screenshot-detail width="200" height="60" out="/tmp/quotas-detail.txt" wait="10":
  #!/usr/bin/env bash
  set -eu
  tmux kill-session -t quotas_snap 2>/dev/null || true
  tmux new-session -d -s quotas_snap -x {{width}} -y {{height}} './target/debug/quotas'
  sleep {{wait}}
  tmux send-keys -t quotas_snap Enter
  sleep 1
  tmux capture-pane -t quotas_snap -p > {{out}}
  tmux kill-session -t quotas_snap
  echo "wrote {{out}}"