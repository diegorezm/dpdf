<img src="assets/icon.svg" width="64" height="64" alt="">

# dpdf

A fast, minimal, local-first PDF reader for Linux, built with Rust and Slint.

dpdf is a calm, keyboard-first reading environment: the page fills the screen, and controls stay hidden until you need them. There is no telemetry, no accounts, and no network access.

> **Status:** early skeleton. The window, theme, and collapsible sidebar exist; PDF rendering (via PDFium) is next.

## Requirements

- Rust (stable) via [rustup](https://rustup.rs)
- On Linux, system libraries for Slint's default backend, e.g. on Debian/Ubuntu:

  ```sh
  sudo apt install libfontconfig1-dev libxkbcommon-dev libwayland-dev
  ```

## Run

```sh
cargo run
```

## Develop

```sh
cargo test                                  # tests
cargo fmt --all                             # format
cargo clippy --all-targets -- -D warnings   # lint
cargo build --release                       # optimized binary: target/release/dpdf
```

## Shortcuts

| Shortcut | Action |
|---|---|
| `Ctrl+B` | Toggle sidebar |

More shortcuts are planned (open, search, jump to page, zoom, navigation); see [AGENTS.md](AGENTS.md).

## Contributing

Read [AGENTS.md](AGENTS.md) for the product philosophy, architecture, and workflow.

## License

[MIT](LICENSE)
