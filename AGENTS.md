# AGENTS.md

Guidance for coding agents (and humans) working on **dpdf**. Read this before changing anything.

---

## 1. What dpdf is and why it exists

`dpdf` is a personal, local-first desktop PDF reader for Windows. It exists because mainstream PDF apps are bloated, slow, visually generic, and full of features that get in the way of reading.

dpdf is **not** an Acrobat/Foxit replacement or a PDF office suite. It is a **fast, minimal, beautiful, keyboard-friendly reader** that feels immediate and stays out of the way. The document is the interface: the page fills the screen, and controls, panels, and overlays stay hidden until needed.

## 2. Product philosophy (non-negotiable)

- **Fast** — opening, scrolling, zooming, searching, and navigating must feel instant.
- **Lightweight** — small binary, low idle memory, no background processes.
- **Simple** — no clutter, no permanent toolbars, no feature overload.
- **Beautiful** — a clean, intentional identity, not a generic desktop look.
- **Local-first** — documents, settings, bookmarks, annotations, and caches stay on the machine.
- **Private** — no telemetry, analytics, tracking, accounts, cloud, or forced updates.
- **Keyboard-first** — every common action has a shortcut.
- **Calm** — no gratuitous animation, notifications, or popups that interrupt reading.

Every feature must earn its place: it must make reading or organizing documents better **without** making the app heavier, slower, or more complicated. "Other PDF apps have it" is not a reason. When in doubt, leave it out and ask.

## 3. Target user

A software developer on Windows who reads technical documentation, papers, manuals, and personal documents. They value performance, privacy, simplicity, customizability, and polish. Primary use case: long, focused reading sessions of local PDFs, jumping between pages and searching text.

The app is personal first, but code should be clean enough to open-source later.

## 4. Technology direction

| Concern | Choice | Why |
|---|---|---|
| Language | **Rust** (stable, edition 2021) | Native performance, memory safety, small binaries. |
| UI | **Slint** | Native, lightweight declarative UI; no browser engine; compiles `.slint` markup into Rust. |
| Rendering | **PDFium** via `pdfium-render` | Mature, fast, accurate rendering and text extraction. |
| Manipulation (later) | `lopdf` | Merge/split/rotate/extract pages without heavy deps. |

**Forbidden:** Electron, Tauri, WebViews, React/Svelte/any JS, browser engines. Performance and native feel are product requirements, not later optimizations.

PDFium is loaded at runtime as `pdfium.dll`, first from the executable's directory, then from the system. `build.rs` downloads the prebuilt DLL matching the `pdfium_8125` feature (`PDFIUM_RELEASE`) once into `target/pdfium/` and copies it next to the built exe. When bumping pdfium-render, keep the feature flag and `PDFIUM_RELEASE` in sync. Tests that need PDFium skip when it can't be loaded.

## 5. MVP features

1. Open local PDF files.
2. Fast, smooth display.
3. Continuous scrolling.
4. Zoom, fit width, fit page.
5. Page navigation; show current page / total.
6. Collapsible page-thumbnail sidebar.
7. Text search.
8. Remember reading position and recent documents.
9. Dark theme by default, light theme option.
10. Minimal settings screen.

## 6. Intentionally deferred (or rejected)

Built after the MVP, because they serve the core use (studying documents):
- Tabs and named sessions (tmux-like: each session has its own tabs; dpdf reattaches to the last one on startup).
- Annotations (highlight, underline, strikethrough, notes) stored outside the PDF, with Markdown export.
- Focus mode, command palette, TOML themes and keybindings, optional vim keys.
- Outline and notes panels in the sidebar, clickable links, page labels, password-protected PDFs.

Customizability is part of dpdf's identity: themes and keys are plain files, and every action is reachable from the palette. New options still have to earn their place; prefer a file-only option over another visible control.

Deferred — may come later, must stay optional, local, and non-intrusive:
- Bookmarks UI.
- Page manipulation (merge, split, rotate, extract) via `lopdf`.
- Presentation mode, writing annotations into the PDF itself.

Rejected by default:
- Permanent toolbars, ribbons, sidebar-heavy layouts.
- Accounts, subscriptions, cloud sync, any network dependency.
- Telemetry, analytics, silent or forced updates.
- AI assistant panels, plugin systems.
- Excessive settings, decorative animations.

## 7. UI direction

```text
┌──────────────────────────────────────────────┐
│  Document title            page 12 / 240  ⚙ │
├──────────┬───────────────────────────────────┤
│ Sidebar  │                                   │
│          │           PDF page                │
│ thumbs   │                                   │
│          │                                   │
└──────────┴───────────────────────────────────┘
```

- Dark-first, restrained palette; light theme is a peer, not an afterthought.
- Generous spacing, subtle 1px borders, no gradients/shadows-as-decoration.
- The reading area dominates. The sidebar is optional and collapsible.
- Overlays (search, jump-to-page, palette, note editor, settings) appear on demand and dismiss with `Esc`.
- One accent color, used sparingly (focus, current page, search hits).
- Design tokens live in `ui/theme.slint`; colors are set from the active theme file (`themes/*.toml`, `config.rs`). Never hard-code colors in components; add a theme key instead (and to every built-in theme).
- Anti-clutter rule: before adding a visible control, ask whether a shortcut or an on-demand overlay would do. Default answer is yes.

## 8. Keyboard-first behavior

Bindings are data, not code: `DEFAULT_KEYS` and `VIM_KEYS` in `src/keymap.rs`, overridable per action in `config.toml` under `[keys]`. Every `Action` has a stable kebab-case name (used in config) and a title (used in the palette and Settings). Multi-key sequences (`g g`) are supported.

Core defaults: `Ctrl+O` open, `Ctrl+K` palette, `Ctrl+F` search, `Ctrl+P` go to page, `Ctrl++/-/0` zoom, `Ctrl+1/2` fit width/page, `Ctrl+B` sidebar, `Ctrl+.` focus mode, `F11` fullscreen, `Ctrl+Tab`/`Ctrl+W` tabs, arrows/PgUp/PgDn/Home/End navigation, `Ctrl+C/H/N` copy/highlight/note, `Esc` close/clear.

(`Cmd` replaces `Ctrl` on macOS if it is ever supported.) Every new user-facing action gets an `Action` variant (so it is bindable and in the palette) and, usually, a default key.

## 9. Performance and resource expectations

- PDF rendering **never** runs on the UI thread. Use a worker thread and send results back via `slint::invoke_from_event_loop` or a `Weak` handle.
- Render only visible pages plus a small look-ahead window during scroll.
- Cache rendered pages in a bounded LRU keyed by `(page, scale, theme)`; evict aggressively. Memory budget is a parameter, not unbounded.
- Thumbnails are small, cheap renders cached separately from full pages.
- Cancel or deprioritize stale render requests when the user scrolls/zooms past them.
- No polling loops, no background work while idle.
- Targets (guidelines, measure before claiming): cold start to window < 300 ms; idle RSS with no document < 50 MB; release binary stays small (`lto`, `strip`, `opt-level`).
- Any change touching rendering, caching, or scrolling should state its performance impact in the commit/PR. Prefer `perf:` commits with before/after numbers.

## 10. Architecture

**Slint is presentation only. Rust owns state and logic.**

```text
src/
  main.rs         // entry: build UI, wire callbacks to App, route worker events
  app.rs          // tabs + session, keys -> actions, palette, settings, themes, overlays
  viewer.rs       // the open document: layout, render cache, search, selection, annotations
  pdf.rs          // PDFium worker thread: open, render, search, select text (no Slint types)
  layout.rs       // pure page-strip geometry, zoom modes
  keymap.rs       // actions, key chords and sequences, default + vim bindings
  palette.rs      // pure fuzzy matching for the command palette
  search.rs       // pure search results and match navigation
  annotations.rs  // annotation model, TOML storage, Markdown export
  config.rs       // config.toml + theme files
  store.rs        // reading history and last session (state.txt)
  nav.rs          // pure page navigation and page-number parsing
  update.rs       // update check and install from GitHub releases
ui/
  app.slint       // main window: header, tabs, reader, overlays
  reader.slint    // page strip, thumbnails sidebar, start screen, tab bar
  palette.slint   // command palette
  settings.slint  // settings sheet
  widgets.slint   // small shared components (buttons, toggle, select, panel)
  structs.slint   // data passed from Rust
  theme.slint     // design tokens (global Theme, colors set from Rust)
themes/           // built-in themes, embedded at compile time
```

The PDF worker processes one small unit at a time (text selection > page render > search one page > thumbnail) and checks for new requests between units, so new requests replace stale work. Tabs share one live document view. The worker keeps the last few documents open and the viewer keeps their exact scroll position and nearby rendered pages, so switching between recent tabs is instant; older tabs reopen from disk, which keeps memory bounded no matter how many tabs are open.

Rules:
- `.slint` files expose properties and callbacks; they contain no business logic beyond trivial layout/visibility bindings.
- `pdf`, `layout`, `nav`, `keymap`, `palette`, `search`, `annotations`, `config`, `store` must not depend on Slint UI types, so they are unit-testable (`keymap` only uses Slint's key constants). Only `main.rs`, `app.rs`, and `viewer.rs` touch Slint types.
- State flows one way: UI callback → `Action` → controller updates state → controller pushes properties to UI.
- Paths: everything user-owned lives in `%APPDATA%\dpdf` (`config.toml`, `themes\`, `annotations\`, `state.txt`); caches go in `%LOCALAPPDATA%\dpdf`. Never write next to the executable, in the document folder, or into the PDF.
- User files must never block startup: parse leniently, fall back to defaults, and report problems in Settings.
- Grow the module tree when needed; do not create empty modules ahead of time.

## 11. Coding standards

- Idiomatic, readable Rust. Small functions, clear names, minimal comments that explain *why*.
- **No `unwrap()` / `expect()` in production paths.** Allowed in tests and for truly impossible invariants documented with a comment. Propagate with `?`.
- Errors: a per-module error enum (`thiserror` if/when warranted) at library boundaries; `anyhow`-style context only at the top level. Show user-facing errors calmly in the UI; never panic on a bad PDF.
- No `unsafe` unless unavoidable and documented with a `// SAFETY:` comment.
- `cargo fmt` clean; `cargo clippy --all-targets -- -D warnings` clean.

**Testing**
- Unit-test meaningful logic: cache eviction, search result handling, settings parsing/defaults, zoom/fit math, page navigation bounds, keymap.
- Keep logic pure so tests need no window or PDFium. Integration tests that require PDFium should skip gracefully when the library is missing.
- Small fixture PDFs go in `tests/fixtures/` (only freely licensed, tiny files).

**Dependency policy**
- Every new dependency needs a clear reason stated in the commit message. Prefer std.
- Disable unneeded default features. Avoid crates that pull in async runtimes, networking, or large transitive trees.
- No dependency that phones home or requires network at runtime.

**Security & privacy**
- No network access, ever, without an explicit future decision recorded here.
- Recorded exception (update check): `update.rs` asks github.com for the latest release on startup (`check_updates = true` by default, off in `config.toml`) and from the palette. It sends nothing about the user, downloads only after the user agrees, verifies the installer against the release's `SHA256SUMS.txt`, and uses Windows' `curl.exe` so there is no HTTP stack in the binary. Portable copies only open the release page.
- Treat PDFs as untrusted input: never panic, bound memory on huge pages, validate page indices.
- Never log document contents or full paths at default log levels.
- Never commit secrets, local configs, or user documents.

## 12. Development workflow

Prerequisites: stable Rust via `rustup` with the MSVC toolchain (`x86_64-pc-windows-msvc`), plus the Visual Studio C++ Build Tools. No other system libraries are needed for Slint.

```sh
cargo run                                   # debug build and launch
cargo test                                  # run tests
cargo fmt --all                             # format
cargo clippy --all-targets -- -D warnings   # lint
cargo build --release                       # optimized build -> target/release/dpdf
```

Run `fmt`, `clippy`, `test`, and `build` before every commit.

## 13. Git and GitHub workflow

- Small, logical commits. One concern per commit. No giant dumps.
- Conventional commits: `feat:`, `fix:`, `docs:`, `chore:`, `refactor:`, `test:`, `perf:`. Imperative mood, lowercase summary, ≤ 72 chars.
- The project must build before every commit; run fmt/clippy when applicable.
- Never commit `target/`, build artifacts, local secrets, `.env`, editor folders (`.vscode/`, `.idea/`), or OS junk.
- `main` must always build. Use feature branches + PRs for non-trivial work.
- Repository: public GitHub repo `dpdf`, MIT licensed. Use the `gh` CLI for GitHub operations; if it is unauthenticated, stop and tell the user rather than using another method.

## 14. Definition of done

A change is done when:

- [ ] It serves the philosophy above and adds no unrequested scope.
- [ ] `cargo build` and `cargo build --release` succeed.
- [ ] `cargo fmt --all -- --check` passes.
- [ ] `cargo clippy --all-targets -- -D warnings` passes.
- [ ] `cargo test` passes; new logic has tests.
- [ ] No `unwrap`/`expect` in production paths; no UI-thread blocking.
- [ ] New user-facing actions have keyboard shortcuts.
- [ ] Docs (README / AGENTS.md) updated if behavior, setup, or principles changed.
- [ ] Committed in logical conventional commits with no artifacts or secrets.
