# Contributing to cordis-rs

Thanks for your interest in contributing! Here's how to get started:

## Development Setup

```bash
# 1. Fork and clone
git clone https://github.com/<your-user>/cordis-rs.git
cd cordis-rs

# 2. Build
cargo build --workspace

# 3. Run tests
cargo test --workspace

# 4. Check formatting
cargo fmt -- --check

# 5. Run clippy
cargo clippy --workspace -- -D warnings
```

## Package Structure

Each package follows this layout:

```
cordis-<name>/
├── Cargo.toml
├── src/
│   ├── lib.rs          # Public re-exports
│   └── <name>.rs       # Main implementation
```

## Commit Convention

We use Conventional Commits:

- `feat(core): add effect system to Fiber`
- `fix(timer): handle disposed intervals correctly`
- `docs(readme): update quick start example`
- `test(events): add parallel dispatch test`
- `refactor(logger): simplify exporter trait`

## Pull Request Process

1. Create a feature branch from `main`
2. Make your changes with tests
3. Run `cargo test --workspace` and `cargo clippy --workspace`
4. Update documentation if needed
5. Submit the PR

## Code of Conduct

Please be respectful and constructive in all interactions.
