# dpdf documentation

How to install, use, customize, and build dpdf. For an overview, see the [README](README.md).

## Install

Download `dpdf-<version>-setup.exe` (installs per user, no admin needed, optionally adds dpdf to "Open with" for PDFs) or the portable `.zip` from [Releases](https://github.com/diegorezm/dpdf/releases).

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
| `Ctrl+B` | Toggle sidebar |
| `Ctrl+Shift+T` / `O` / `A` | Sidebar: thumbnails / outline / notes |
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
font_size = 14                   # 10 to 22
density = "comfortable"          # compact, comfortable, spacious
corners = "rounded"              # sharp, rounded, round
thumbnail_size = "medium"        # small, medium, large
vim = true
check_updates = false            # don't ask GitHub for new releases on startup
sidebar_button = false           # hide the header button that reopens the sidebar
notes_folder = 'C:\Users\me\Vault\PDF notes'  # Ctrl+Shift+E writes here, no dialog

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

## Building from source

### Requirements

- Rust (stable) via [rustup](https://rustup.rs)
- Windows with the MSVC toolchain (`rustup default stable-x86_64-pc-windows-msvc`) and the Visual Studio C++ Build Tools

### Run

```sh
cargo run                      # then Ctrl+O to pick files
cargo run -- a.pdf b.pdf       # or open some directly, one tab each
```

PDF rendering uses [PDFium](https://pdfium.googlesource.com/pdfium/). The first build downloads a prebuilt `pdfium.dll` (from [pdfium-binaries](https://github.com/bblanchon/pdfium-binaries)) using the `curl` and `tar` that ship with Windows, caches it in `target/pdfium/`, and copies it next to `dpdf.exe`. At runtime the app only contacts github.com to check for updates (`check_updates` in `config.toml`).

To distribute dpdf, ship `dpdf.exe` and `pdfium.dll` together from `target/release/`. Pushing a `v*` tag runs the `release` workflow, which builds both, a zip, and an [Inno Setup](https://jrsoftware.org/isinfo.php) installer (`installer/dpdf.iss`), and publishes a GitHub release.

### Develop

```sh
cargo test                                  # tests
cargo fmt --all                             # format
cargo clippy --all-targets -- -D warnings   # lint
cargo build --release                       # optimized binary: target/release/dpdf.exe
```

## Performance

Measured on a 1,088-page PDF (release build, Windows 11, `cargo test --release bench_document -- --ignored --nocapture` with `DPDF_BENCH_PDF` set):

| | |
|---|---|
| Open document | 9 ms |
| Read outline (88 entries) | 2 ms |
| Render a page at 1600 px | 12 ms average |
| Render a thumbnail | 2 ms average |
| Search every page | 1.2 s (in the background, one page at a time; results stream in) |
| Read page labels | 0.3 s (in the background, after everything else) |
| Window shown after launch | ~30 ms warm, ~0.4 s cold |
| Memory, no document | ~130 MB working set |
| Memory, 1,088-page document | ~210 MB working set |
| `dpdf.exe` / `pdfium.dll` | 12 MB / 7.2 MB |

Idle memory is above the 50 MB goal in AGENTS.md and hasn't been profiled yet (the OpenGL renderer and GPU driver are the likely bulk). With a document open it grows only by the bounded page and thumbnail caches.
