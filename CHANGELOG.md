# Changelog

All notable changes to dpdf. Versions follow [Semantic Versioning](https://semver.org/).

## Unreleased

### Added

- **Highlight colors with a meaning.** Each color has a name (yellow
  *Important*, red *Trap*, and so on), set in `config.toml` under
  `[color_names]`. Keys `1`–`5` highlight the selection or recolor the
  clicked highlight, and page thumbnails show the colors on each page.
- **Notes filter.** The notes panel can hide colors.
- **Section progress.** Outline sections can be marked read and reviewed
  (click the box or `Ctrl+L`); the outline and the home screen show how far
  each document is.
- **Export sheet** (`Ctrl+E`): a Markdown summary grouped by section and
  color, the Obsidian format, a file Anki imports as cards, or CSV.
- **Study timer** in the header, counting the time you actually read, per
  day and document (`Ctrl+T` pauses it; `study_timer = false` hides it).
- **Home screen** lists the session's tabs with progress, notes, and time.
- **Session search** (`Ctrl+Shift+F`) across every tab, ignoring accents.
- **Go to article**: `art. 37` in Go to page jumps to that article.

## 1.0.4 — 2026-11-09

Version 1.0.3 was skipped: its performance work ships here, together with
the formula fix.

### Fixed

- **Copying formulas from Word-made PDFs.** Equations exported by Word use
  a Cambria Math subset whose text map turns most symbols into spaces, so
  copying `x² − 4x + 2 = 0` gave blanks (in other readers too). dpdf now
  reads the glyphs the page actually draws and names them through the
  font: the copy embedded in the PDF first, then the same font installed on
  Windows. Exponents and indices come out as `²`, `₁`, and so on; math
  italic letters as plain ones. Two-dimensional layouts such as stacked
  fractions are copied in reading order, without fraction bars.

### Performance

Measured on a 1088-page book (`cargo test --release bench_document`):

| | 1.0.2 | 1.0.4 |
|---|---|---|
| Dark-mode inversion of a 1600 px page | 6.6 ms | 2.7 ms |
| Page labels after opening | 305 ms | 0.6 ms |
| Second search through the whole book | ~1200 ms | 577 ms |

- Page pixels are converted on the render thread; the UI thread no longer
  copies a full page per render.
- Pages and thumbnails are kept as RGB: a quarter less memory each.
- A finished render updates only its own page instead of rebuilding every
  visible page.
- Rendered pages share a 192 MB memory budget, evicting the pages farthest
  from view first; tabs kept in the background hold only the pages that
  were on screen.
- One render is capped at 24 megapixels and 16384 px per side, so deep
  zoom no longer asks for 300 MB bitmaps; past that the page is stretched.
- Pages not rendered yet show their thumbnail, or a quarter-size preview,
  instead of a blank.
- Zooming in quick steps (Ctrl+wheel, a held key) renders only the last
  step.
- The reading position is saved at most every two seconds while scrolling,
  instead of on every page; closing, switching tabs, and quitting still
  save at once.
- Typing a search narrows the previous one, and pages already searched are
  skipped quickly when they can't match.
- Page labels are read without loading every page, and kept per document.
- Text selection no longer copies a page's characters on every drag event.
- Thumbnails skip form fields.

## 1.0.2 — 2026-11-09

### Performance

- Switching between recent tabs is instant: the document stays open and
  its rendered pages and exact position are kept.

## 1.0.1 — 2026-11-09

### Added

- Update check against GitHub releases, with an offer to install (off with
  `check_updates = false`).
- Double-click selects a line; note tooltips; Obsidian-friendly notes
  export.
- A header button to reopen the closed sidebar.

### Fixed

- Keyboard focus is kept after closing the sidebar with its button.

## 1.0.0 — 2026-11-09

First release.
