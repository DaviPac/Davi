# Davi

A lightweight, fast, cross-platform API client written in Rust on top of
[GPUI](https://www.gpui.rs/), Zed's GPU-accelerated UI framework. Collections
are plain-text Bruno `.bru` files on disk, so they version cleanly in Git and
stay compatible with [Bruno](https://www.usebruno.com/).

```
cargo run --release -- examples/sample-collection   # or DAVI_COLLECTION=<dir>
```

| Shortcut (⌘ on macOS)  | Action                                  |
| ---------------------- | --------------------------------------- |
| `Ctrl+P`               | Quick-open request (fuzzy search)        |
| `Ctrl+Enter`           | Send / cancel the active request         |
| `Ctrl+S`               | Save the active request to its `.bru`    |
| `Ctrl+W`               | Close tab                                |
| `Ctrl+E`               | Cycle environment                        |

## Workspace architecture

```
                 ┌────────────────────────────────────────────┐
                 │ davi-ui  (bin: davi)                       │
                 │  GPUI views · app state · keymap · palette │
                 └──────────────┬──────────────┬──────────────┘
                                │              │ RequestHandle (executor-agnostic)
                                │              ▼
                                │   ┌──────────────────────────┐
                                │   │ davi-net                 │
                                │   │  prepare → reqwest/tokio │
                                │   │  streaming · metrics     │
                                │   └────────────┬─────────────┘
                                ▼                ▼
                 ┌────────────────────────────────────────────┐
                 │ davi-core                                  │
                 │  model · .bru parser/writer · env/vars ·   │
                 │  collection loader       (no async, no UI) │
                 └────────────────────────────────────────────┘
```

Dependencies point strictly downwards. `davi-core` builds in seconds and is
reusable by a future CLI test runner; only `davi-ui` pulls in GPUI.

### `davi-core`: data model and `.bru` format

* **`bru::parse` / `bru::write`**: a generic block AST (`Dict`, `Text`, `List`).
  The parser is two layers: `nom` combinators recognise single-line tokens
  (block headers, `~key: value` entries, `'''` multi-line values, list items),
  and a small line-oriented driver handles block bodies. Errors carry a
  1-based line and column plus the block they occurred in. It accepts CRLF
  and a BOM.
* **`HttpRequest::from_bru` / `to_bru`**: lowering into a typed model (meta,
  method + URL, query/path params, headers, auth, every body mode, pre/post
  vars, assertions, scripts, tests, docs). Unknown blocks and unsupported
  auth modes go into `extra_blocks`, so saving never drops data. The writer
  mirrors Bruno's formatting, so files Bruno wrote round-trip **byte for
  byte** (this is tested).
* **`env`**: environments (`vars` + `vars:secret`) and a layered `VarScope`.
  `{{var}}` interpolation is one pass, resolves nested references up to depth
  8 (which also guards against cycles), and supports `{{process.env.X}}`. It
  returns `Cow::Borrowed` when there is nothing to replace.
* **`collection`**: walks a collection directory into a sidebar tree of
  `RequestSummary` values (name, method, seq, path), ordered by `folder.bru`
  and `seq`. Full requests are parsed only when a tab is opened. Saves are
  atomic (write to a temp file, then rename).

### `davi-net`: HTTP engine

* **`prepare()`**: a pure function from `HttpRequest + VarScope` to a
  `PreparedRequest`. It interpolates the request, substitutes `:path` params,
  applies Bearer, Basic or API-key auth (header or query), and builds the body
  (raw, form, multipart with `@file(...)`, or a GraphQL envelope). It is
  unit-testable without a network.
* **`HttpEngine`** owns one shared `reqwest::Client` (connection pool, cookie
  jar, gzip/br/zstd/deflate, HTTP/2, rustls) and a **dedicated 2-thread tokio
  runtime**. GPUI has its own executor and no tokio reactor, so the engine
  returns a `RequestHandle` backed by tokio `oneshot`/`watch` channels, which
  any executor can await. Tokio never leaks into the UI crate.
* Bodies are streamed chunk by chunk. While streaming, the engine publishes
  `Progress`, measures TTFB and total time, records header and body sizes,
  and stops at a configurable memory cap (`truncated: true`). Requests can be
  cancelled.

### `davi-ui`: GPUI front-end

* `Workspace` (root entity): sidebar | tab bar over (request panel |
  response panel), a status bar, and the palette overlay. Global actions
  (`SendRequest`, `SaveRequest`, ...) are bound with `secondary-*` keys, so
  they map to ⌘ on macOS and Ctrl elsewhere.
* The sidebar and the response body use **`uniform_list`** virtualization, so
  only visible rows are laid out. A multi-MB response scrolls like a small one.
* Responses are pretty-printed, split into lines and JSON-highlighted **on
  GPUI's background executor**. `render` only slices the visible lines and
  draws them with `StyledText` highlights.
* `CommandPalette` is its own entity with its own focus. It communicates only
  through `PaletteEvent` (the `EventEmitter`/`subscribe_in` pattern), which is
  how the other panels will be split out as they gain editors.
* Each tab tracks its `saved` vs. live `HttpRequest`. Structural equality
  drives the dirty indicator (●).

### Performance strategy (targets: < 50 MB idle, instant launch)

* No webview or JS runtime. GPUI renders straight to Metal, Vulkan or DX.
* Tree-only loading at startup: summaries, not full requests.
* Network threads are created once and sit idle, using no CPU.
* Release profile: fat LTO, `codegen-units = 1`, `panic = "abort"`, stripped
  symbols. Dependencies use `opt-level = 2` even in dev builds so debug runs
  stay smooth.

## Building

Rust 1.88+ (edition 2024).

**Linux** also needs the GPUI system libraries, e.g. on Debian/Ubuntu:

```
sudo apt install libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev \
  libfontconfig-dev libfreetype-dev libvulkan1 libxcb1-dev
```

> `Cargo.lock` pins `libc` to 0.2.189: `libc 0.2.190` removed `ENOATTR`,
> which breaks `xattr 0.2` (pulled in by `gpui 0.2.2` → `gpui_http_client`).
> Don't run a bare `cargo update` until upstream is fixed.

```
cargo test --workspace
cargo clippy --workspace --all-targets
```

## Roadmap

1. Text input / code editor component (URL bar, key-value tables, bodies),
   then make the editor panels their own entities.
2. Collection and folder-level headers, auth (`inherit`) and vars; `.env`.
3. File watching (`notify`) to live-reload `.bru` files changed by Git.
4. Tree-sitter highlighting for XML/HTML, response search, horizontal scroll.
5. Pre/post scripts and assertions (embedded JS engine, evaluated lazily).
