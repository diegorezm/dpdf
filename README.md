<img src="assets/icon.svg" width="64" height="64" alt="">

# dpdf

A fast, minimal, local-first PDF reader for Windows, built with Rust and Slint.

dpdf is a calm, keyboard-first reading environment: the page fills the screen, and controls stay hidden until you need them. It is built to be customized: themes and keybindings are plain TOML files. There is no telemetry and no accounts; the only network request is an update check against GitHub releases, which asks before installing anything and can be turned off.

<p align="center"><img src="site/shots/reader.png" alt="dpdf with a document, page thumbnails, and highlights"></p>

**[Website](https://diegorezm.github.io/dpdf/) · [Download](https://github.com/diegorezm/dpdf/releases/latest) · [Docs](DOCS.md)**

## Features

- **Reading:** continuous scrolling, fit width / fit page / custom zoom (also `Ctrl`+wheel), go to page (numbers or page labels like `iv`), clickable links, fullscreen, and a **focus mode** that hides everything but the page.
- **Sidebar:** page thumbnails, the document's outline, or a list of your notes; click to jump.
- **Tabs and sessions:** open several PDFs at once. Group them into named sessions, like tmux: one for your thesis, one for a course. Switch with `Ctrl+Shift+S`; dpdf reattaches to the last session on startup, each tab at the page and zoom you left it.
- **Annotations:** select text (across pages, too) to highlight it (5 colors), underline, strike through, attach a note, or copy it. Double-click selects a line; hovering a note shows it. **Export your notes to Obsidian-ready Markdown:** properties, one callout per quote linking to its page (`[[book.pdf#page=12|p. 12, line 7]]`), and stable `^dpdf-<id>` block ids you can link to from other notes. Notes are stored next to your settings; the PDF is never modified.
- **Search:** in-document text search with match highlighting.
- **Command palette** (`Ctrl+K`): every command, open tab, recent file, and theme, with fuzzy matching. Type a number to jump to that page.
- **Customizable:** themes, interface font, density, and keybindings in TOML, optional **vim keys**, inverted page colors for night reading.
- Opens password-protected PDFs; drop files on the window to open them.

## Screenshots

| | |
|---|---|
| ![Annotations and notes](site/shots/notes.png) | ![Outline in the sidebar](site/shots/outline.png) |
| Highlights and notes | Outline, thumbnails, or notes in the sidebar |
| ![Search](site/shots/search.png) | ![Command palette](site/shots/palette.png) |
| Search with highlighted matches | Command palette |
| ![Focus mode](site/shots/focus.png) | ![Paper theme](site/shots/themes.png) |
| Focus mode | Light Paper theme, one of ten built in |

## Install

Download the installer or the portable `.zip` from [Releases](https://github.com/diegorezm/dpdf/releases/latest). Setup, shortcuts, customization, and building from source are in [DOCS.md](DOCS.md).

## Contributing

Read [AGENTS.md](AGENTS.md) for the product philosophy, architecture, and workflow, and [DOCS.md](DOCS.md#building-from-source) to build and test.

## License

[MIT](LICENSE)
