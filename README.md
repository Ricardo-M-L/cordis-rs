# cordis-rs

> 🦀 Rust port of [cordis](https://github.com/cordisjs/cordis) — a lightweight, flexible, and production-ready framework for building applications.

[![Crates.io](https://img.shields.io/crates/v/cordis-core.svg)](https://crates.io/crates/cordis-core)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Rust 1.70+](https://img.shields.io/badge/Rust-1.70%2B-orange.svg)](https://www.rust-lang.org)
[![CI](https://img.shields.io/badge/CI-passing-green.svg)](https://github.com/Ricardo-M-L/cordis-rs/actions)

## Workspace

```
cordis-rs/
├── Cargo.toml                    # Workspace root
├── cordis-core/                  # Core framework (Fiber, Context, Events, Registry, Reflect, Logger, Utils)
├── cordis-timer/                 # Timer service (timeout, interval, throttle, debounce)
├── cordis-logger-console/        # Console log exporter with ANSI colors
├── cordis-utils/                 # Shared utilities (List, merge_configs, format_date)
├── cordis-group/                 # Entry grouping
├── cordis-include/               # Config file loading and patch system
├── cordis-loader/                # Module loader (Entry, Group, ModuleLoader)
├── cordis-hmr/                   # Hot Module Replacement
└── cordis-create/                # CLI scaffolding for new projects
```

## Quick Start

```bash
# Build the entire workspace
cargo build --workspace

# Run all tests
cargo test --workspace

# Run checks (no output on success)
cargo check --workspace
```

## Features

- **Fiber** — Lightweight units of work with effect/disposable lifecycle and state machine (Pending → Loading → Active → Failed → Disposed)
- **Context** — Dependency-injection container with typed store and isolation
- **Events** — Pub/sub bus with `emit`, `serial`, `parallel`, `bail`, and `waterfall` dispatch strategies
- **Registry** — Plugin system with dependency injection and schema validation
- **Reflect** — Type-aware metadata store with `provide`/`get`/`delete`
- **Logger** — Structured logging with `%s`/`%o`/`%d` formatting and pluggable exporters
- **Timer** — Async `timeout`, `interval`, `throttle`, and `debounce` with dispose support
- **HMR** — Hot Module Replacement with dependency tracking
- **Loader** — Entry tree loading with groups, isolates, and module resolution
- **Create** — CLI scaffolding for new Cordis projects

## License

MIT — see [LICENSE](./LICENSE).

## Related

- [cordis (TypeScript)](https://github.com/cordisjs/cordis) — The original JavaScript/TypeScript framework
