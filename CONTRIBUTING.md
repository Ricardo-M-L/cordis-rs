# Contributing to cordis-rs

Thanks for your interest in contributing! Here's how to get started:

## Development Setup

Install Rust 1.85 or newer with the `rustfmt` and `clippy` components.

```bash
# 1. Fork and clone
git clone https://github.com/<your-user>/cordis-rs.git
cd cordis-rs

# 2. Build
cargo check --workspace --all-targets --locked

# 3. Run tests
cargo test --workspace --all-targets --locked

# 4. Check formatting
cargo fmt --all -- --check

# 5. Run clippy
cargo clippy --workspace --all-targets --locked -- -D warnings
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

1. Create a feature branch from the repository's default branch
2. Make your changes with tests
3. Run the complete formatting, check, test, and Clippy commands above
4. Update documentation if needed
5. Submit the PR

## Code of Conduct

Please be respectful and constructive in all interactions.
