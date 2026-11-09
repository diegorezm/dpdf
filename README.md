<img src="assets/icon.svg" width="64" height="64" alt="">

# dpdf

A fast, minimal, local-first PDF reader for Windows, built with Rust and Slint.

dpdf is a calm, keyboard-first reading environment: the page fills the screen, and controls stay hidden until you need them. It is built to be customized: themes and keybindings are plain TOML files. There is no telemetry, no accounts, and no network access.

## Features

- **Reading:** continuous scrolling, fit width / fit page / custom zoom (also `Ctrl`+wheel), thumbnails, go to page, fullscreen, and a **focus mode** that hides everything but the page.
- **Tabs and sessions:** open several PDFs at once. Group them into named sessions, like tmux: one for your thesis, one for a course. Switch with `Ctrl+Shift+S`; dpdf reattaches to the last session on startup, each tab at the page and zoom you left it.
- **Annotations:** select text to highlight it (5 colors), underline, strike through, attach a note, or copy it. **Export your notes as Markdown.** Notes are stored next to your settings; the PDF is never modified.
- **Search:** in-document text search with match highlighting.
- **Command palette** (`Ctrl+K`): every command, open tab, recent file, and theme, with fuzzy matching. Type a number to jump to that page.
- **Customizable:** themes and keybindings in TOML, optional **vim keys**, inverted page colors for night reading.

## Requirements

- Rust (stable) via [rustup](https://rustup.rs)
- Windows with the MSVC toolchain (`rustup default stable-x86_64-pc-windows-msvc`) and the Visual Studio C++ Build Tools

## Run

```sh
cargo run                      # then Ctrl+O to pick files
cargo run -- a.pdf b.pdf       # or open some directly, one tab each
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

The defaults. All of them can be changed (see [Customizing](#customizing)), and Settings → Keybindings lists every action.

| Shortcut | Action |
|---|---|
| `Ctrl+O` | Open PDFs (in new tabs) |
| `Ctrl+K` / `Ctrl+Shift+P` | Command palette |
| `Ctrl+F` | Search (`Enter` / `Shift+Enter` for next / previous match) |
| `Ctrl+P` | Go to page |
| `Ctrl++` / `Ctrl+-` / `Ctrl`+wheel | Zoom in / out |
| `Ctrl+0` or `Ctrl+1` / `Ctrl+2` | Fit width / fit page |
| `Ctrl+B` | Toggle thumbnail sidebar |
| `Ctrl+.` | Focus mode |
| `F11` | Fullscreen |
| `Ctrl+Tab` / `Ctrl+Shift+Tab` / `Ctrl+W` | Next / previous / close tab |
| `Alt+Home` | Home screen (tabs stay open) |
| `Ctrl+Shift+S` / `Ctrl+Shift+N` | Switch session / new session |
| `←` `→` `PgUp` `PgDn` / `↑` `↓` | Previous / next page / scroll |
| `Home` / `End` | First / last page |
| `Ctrl+C` / `Ctrl+H` / `Ctrl+N` | Copy / highlight / add a note to the selection |
| `Ctrl+Z` / `Ctrl+Y` | Undo / redo annotation changes |
| `Ctrl+Shift+E` | Export notes as Markdown |
| `Ctrl+,` | Settings |
| `Esc` | Close overlay, clear selection, leave fullscreen or focus mode |

With **vim keys** on: `j` `k` `h` `l` scroll, `Ctrl+D` / `Ctrl+U` half screen, `J` / `K` next / previous page, `gg` / `G` first / last page, `/` search, `n` / `N` next / previous match, `:` palette, `gt` / `gT` tabs, `zi` `zo` `zw` `zp` zoom, `y` copy, `u` / `Ctrl+R` undo / redo, `t` sidebar.

## Customizing

Everything lives in `%APPDATA%\dpdf\` (Settings → General → Data folder):

| File | What |
|---|---|
| `config.toml` | Preferences and keybindings. Settings writes here too. |
| `themes\*.toml` | Your themes. Start from any file in this repo's [`themes/`](themes) folder. |
| `annotations\*.toml` | Highlights and notes, one file per document. |
| `state.txt` | Recent files, reading positions, and sessions with their tabs. |

Font, keybindings, and more in `config.toml`:

```toml
font = "Segoe UI Variable Text"  # any installed font, e.g. "Inter"
font_size = 14
vim = true

[keys]
open = ["ctrl+o"]
first-page = ["home", "g g"]   # sequences are space-separated
search = []                     # an empty list unbinds
```

A theme:

```toml
name = "My Theme"
dark = true             # missing colors fall back to the built-in dark theme
background = "#101418"
accent = "#ff9e64"
```

After editing, run **Reload config and themes** from the palette.

## Contributing

Read [AGENTS.md](AGENTS.md) for the product philosophy, architecture, and workflow.

## License

[MIT](LICENSE)
