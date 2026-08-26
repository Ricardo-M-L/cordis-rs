# cordis-rs

An experimental Rust adaptation of [Cordis](https://github.com/cordisjs/cordis), focused on typed scopes, deterministic plugin cleanup, event dispatch, configuration loading, and file-backed reload signals.

[![CI](https://github.com/Ricardo-M-L/cordis-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/Ricardo-M-L/cordis-rs/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![Rust 1.85+](https://img.shields.io/badge/Rust-1.85%2B-orange.svg)](https://www.rust-lang.org)

> This project is not a drop-in API-compatible port of the JavaScript framework. It uses explicit Rust types and registered module factories in places where Cordis relies on JavaScript proxies or dynamic module loading. The workspace is currently marked `publish = false` because several `cordis-*` crate names are owned by another crates.io project.

## Implemented behavior

- **Fiber**: validated states, typed effect results, synchronous/asynchronous factories, RAII handles, and LIFO cleanup.
- **Context**: typed hierarchical scopes with inherited reads, local writes, deletion masks, and explicit isolation identities.
- **Events**: synchronous and asynchronous listeners, listener removal, serial/bail/waterfall dispatch, and concurrent parallel dispatch.
- **Registry**: plugin validation, named dependency checks, duplicate protection, and rollback-safe unload.
- **Logger**: `%s`, `%d`, `%o` formatting, Unicode-safe truncation, bounded message history, and reentrant exporters.
- **Timer**: stoppable timeout/interval streams and reusable debounce/throttle handles.
- **Include**: JSON, YAML, and TOML file loading plus safe object/array patches.
- **Loader**: entry-tree activation through statically registered Rust module factories, scoped contexts, disabled-tree propagation, unload, reload, and rollback.
- **HMR**: recursive operating-system file watching, debounce, ignored paths, callbacks, and transitive dependency reload events. The application remains responsible for mapping reload events to its module factories.
- **Create**: executable project generator with overwrite protection, built-in or Git templates, optional Git initialization, and release-profile generation.

## Workspace

```text
cordis-core/             Core runtime
cordis-timer/            Timeout, interval, debounce, throttle
cordis-logger-console/   ANSI console exporter
cordis-utils/            Shared collections and configuration helpers
cordis-group/            Entry grouping
cordis-include/          JSON/YAML/TOML loading and patching
cordis-loader/           Entry tree and registered module factories
cordis-hmr/              File watcher and reload dependency graph
cordis-create/           Project scaffolding library and CLI
```

## Build and test

Rust 1.85 or newer is required.

```bash
cargo fmt --all -- --check
cargo check --workspace --all-targets --locked
cargo test --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

## Fiber cleanup

```rust
use cordis_core::{disposer, Fiber};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

let fiber = Fiber::new();
let cleaned = Arc::new(AtomicBool::new(false));
let cleanup_flag = Arc::clone(&cleaned);
let _effect = fiber.effect(move || disposer(move || {
    cleanup_flag.store(true, Ordering::SeqCst);
}));

fiber.dispose();
assert!(cleaned.load(Ordering::SeqCst));
```

## Project generator

```bash
cargo run -p cordis-create -- my-cordis-app --target /tmp/my-cordis-app --git
```

Existing non-empty directories are preserved unless `--force` is explicitly supplied.

## HMR boundary

`cordis-hmr` performs real filesystem observation and emits `Changed`, `Removed`, and transitive `Reload` events. Rust cannot safely unload arbitrary statically linked code. Applications should register module factories with `cordis-loader` and call `Loader::reload()` in response to an accepted reload event.

## License

MIT — see [LICENSE](LICENSE).
