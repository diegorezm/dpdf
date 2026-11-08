<img src="assets/icon.svg" width="64" height="64" alt="">

# dpdf

A fast, minimal, local-first PDF reader for Windows, built with Rust and Slint.

dpdf is a calm, keyboard-first reading environment: the page fills the screen, and controls stay hidden until you need them. There is no telemetry, no accounts, and no network access.

> **Status:** early. You can open a PDF, scroll through it continuously, and zoom; thumbnails, search, and remembering your place are next.

## Requirements

- Rust (stable) via [rustup](https://rustup.rs)
- Windows with the MSVC toolchain (`rustup default stable-x86_64-pc-windows-msvc`) and the Visual Studio C++ Build Tools

## Run

```sh
cargo run                    # then Ctrl+O to pick a file
cargo run -- path/to/doc.pdf # or open one directly
```

PDF rendering uses [PDFium](https://pdfium.googlesource.com/pdfium/). The first build downloads a prebuilt `pdfium.dll` (from [pdfium-binaries](https://github.com/bblanchon/pdfium-binaries)) using the `curl` and `tar` that ship with Windows, caches it in `target/pdfium/`, and copies it next to `dpdf.exe`. The app itself never uses the network.

To distribute dpdf, ship `dpdf.exe` and `pdfium.dll` together from `target/release/`.

## Develop

```sh
cargo test                                  # tests
cargo fmt --all                             # format
cargo clippy --all-targets -- -D warnings   # lint
cargo build --release                       # optimized binary: target/release/dpdf.exe
```

## Shortcuts

| Shortcut | Action |
|---|---|
| `Ctrl+O` | Open PDF |
| `Ctrl+B` | Toggle sidebar |
| `Left` / `PageUp` | Previous page |
| `Right` / `PageDown` | Next page |
| `Home` / `End` | First / last page |
| `Ctrl++` / `Ctrl+-` | Zoom in / out |
| `Ctrl+0` | Reset zoom (fit width) |

Mouse wheel scrolls. More shortcuts are planned (search, jump to page); see [AGENTS.md](AGENTS.md).

## Contributing

Read [AGENTS.md](AGENTS.md) for the product philosophy, architecture, and workflow.

## License

[MIT](LICENSE)
