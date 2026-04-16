# CLAUDE.md — Agent instructions for quotas

## Build & Test

```bash
cargo build          # debug build
cargo test           # run all tests
cargo clippy -- -D warnings   # lint, must pass clean
just install         # install release binary to ~/.cargo/bin
```

## TUI Layout Testing

The TUI dashboard renders quota cards in a grid. Cards are laid out in a
flow-based grid with variable column spanning (MiniMax = 2 cols) and optional
vertical spanning.

### Quick snapshot (no tmux, no API keys, instant)

Renders from the local cache (or empty AuthRequired cards if no cache exists).

```bash
# Default: 160x50
cargo run -- --snap

# Custom size
cargo run -- --snap --snap-width 120 --snap-height 40

# Write to file
cargo run -- --snap --snap-width 200 --snap-height 60 --snap-output /tmp/snap.txt
```

### Full multi-size capture (requires tmux + API keys)

```bash
just screenshots-multi
```

Captures at 5 standard viewport sizes into `screenshots/`:
- `snap-200x60` — wide+tall (desktop fullscreen)
- `snap-160x50` — wide+medium (standard terminal)
- `snap-120x40` — medium (split pane)
- `snap-80x30` — narrow+tall
- `snap-80x20` — narrow+short (pagination case)

Each size produces `.txt` (plain) and `.ansi` (colored) files.

### Single-size capture (requires tmux)

```bash
just screenshot 160 50 /tmp/snap.txt        # grid view
just screenshot-detail 160 50 /tmp/detail.txt  # detail view
```

### Design iteration workflow

1. Make code changes
2. `cargo build -q`
3. Capture snapshots: `cargo run -- --snap --snap-output screenshots/iter9.txt`
   or `just screenshots-multi` for full set with live data
4. Compare with previous `screenshots/iter*.txt`
5. Log observations in `screenshots/log.md` as "Iter N"
6. Commit snapshot files for regression tracking

### What to check at each size

- **Borders**: box-drawing chars form closed rectangles, no broken lines
- **Overflow**: no text clips past card boundaries
- **Row heights**: cards in the same row have equal height
- **Card spacing**: MiniMax gets proportionally more space; short cards don't
  waste excessive vertical space
- **Pace indicators**: compact (1-char icon on header), full badge only with spare rows
- **Pagination**: when cards don't fit on one page, page indicator shows and
  navigation works

## Config

User config: `~/.config/quotas/config.toml` (or `$XDG_CONFIG_HOME/quotas/config.toml`).

```toml
[ui]
show_all_windows = true     # reveal auto-hidden windows (e.g. billing_cycle)
vertical_spanning = true    # experimental: MiniMax as 2x2 card
```
