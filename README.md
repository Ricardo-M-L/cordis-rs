# cordis-rs

An experimental Rust adaptation of [Cordis](https://github.com/cordisjs/cordis), focused on typed scopes, deterministic plugin cleanup, event dispatch, configuration loading, and file-backed reload signals.

[![CI](https://github.com/Ricardo-M-L/cordis-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/Ricardo-M-L/cordis-rs/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![Rust 1.85+](https://img.shields.io/badge/Rust-1.85%2B-orange.svg)](https://www.rust-lang.org)

> This project is not a drop-in API-compatible port of the JavaScript framework. It uses explicit Rust types and registered module factories in places where Cordis relies on JavaScript proxies or dynamic module loading. The workspace is currently marked `publish = false` because several `cordis-*` crate names are owned by another crates.io project.

## Implemented behavior

- **Fiber**: validated states, typed effect results, synchronous/asynchronous factories, RAII handles, and LIFO cleanup.
- **Context**: typed hierarchical scopes, per-service isolation, layered configuration intercepts, and explicit runtime binding.
- **Events**: scoped synchronous/asynchronous listeners, context filtering, Fiber-owned cleanup, serial/bail/waterfall dispatch, and concurrent parallel dispatch.
- **Registry**: isolated plugin/service slots, Service init/check integration, named dependency checks, duplicate protection, and staged replacement.
- **Logger**: `%s`, `%d`, `%o` formatting, Unicode-safe truncation, bounded message history, and reentrant exporters.
- **Timer**: stoppable timeout/interval streams and reusable debounce/throttle handles.
- **Include**: JSON, YAML, and TOML file loading plus safe object/array patches, bounded file/patch limits, and optional strict path behavior.
- **Loader**: entry-tree activation through statically registered Rust module factories, resolved Context intercepts, scoped contexts, disabled-tree propagation, unload, staged reload, and side-effect rollback.
- **HMR**: recursive operating-system file watching, debounce, ignored paths, callbacks, bounded event queue, and transitive dependency reload events with panic-safe callback execution. The application remains responsible for mapping reload events to its module factories.
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

To generate a project that compiles against a local checkout or a released version:

```bash
cargo run -p cordis-create -- my-cordis-app --core-path ../cordis-core
cargo run -p cordis-create -- my-cordis-app --core-version 0.1.0
```

Without these flags, `cordis-create` keeps the existing behavior of using the repository git source.

Existing non-empty directories are preserved unless `--force` is explicitly supplied.

## Runtime integration boundary

`Loader::with_runtime()` binds a root `CordisContext` to one `RegistryService` and shared event bus. Each plugin is prepared in a Loading `Fiber`; services and listeners registered during `Plugin::apply()` remain hidden until activation and are removed with that Fiber. Per-name isolation labels key both plugins and services, while Context intercepts are resolved into the configuration passed to module factories. Reload stages a new Fiber and cleans all of its effects on failure before retaining the old runtime.

This explicit lifecycle replaces Cordis' JavaScript Proxy-based service lookup. It does not implement dynamic JavaScript module linking or offer drop-in API compatibility.

## HMR boundary

`cordis-hmr` performs real filesystem observation and emits `Changed`, `Removed`, and transitive `Reload` events. Rust cannot safely unload arbitrary statically linked code. Applications should register module factories with `cordis-loader` and call `Loader::reload()` in response to an accepted reload event.

## HMR backpressure and observability

Since callbacks can be slow, `cordis-hmr` uses a bounded queue (`queue_capacity`, default `1024`) between the watcher callback and worker thread. Slow consumers therefore drop events instead of blocking file-system processing.

```rust
use cordis_hmr::{Hmr, HmrConfig};

let hmr = Hmr::new(
    "app",
    HmrConfig {
        root: Some("./src".into()),
        base: Some("./src".into()),
        debounce: 100,
        ignored: vec![".DS_Store".into()],
        queue_capacity: 1024,
    },
);

hmr.watch().unwrap();
hmr.on_event(|event| println!("{event}"));

let stats = hmr.stats();
assert!(stats.queue_depth <= stats.queue_capacity);
```

Use `stats()` for SRE diagnostics (`total_received`, `total_emitted`, `total_dropped`, `total_errors`, `callback_panics`, and queue depth/peak).

## Include hardening

`cordis-include` adds security-oriented options for configuration loading:

- `max_file_bytes` bounds input size (default 1MB)
- `max_patch_depth` bounds path depth (default 64)
- `strict` mode for scalar-segment safety in intermediate path traversal

```rust
use cordis_include::{IncludePlugin, Patch};
use serde_json::json;
use std::collections::HashMap;

let plugin = IncludePlugin::with_options(
    "app-config",
    vec![Patch::new("server.port", json!(3000))],
    2 * 1024 * 1024,
    32,
    true,
);

let mut config: HashMap<String, serde_json::Value> = [
    ("server".to_string(), json!({"port": 80})),
].into_iter().collect();

plugin.apply_patches(&mut config).expect("apply patches");
```

## License

MIT — see [LICENSE](LICENSE).
