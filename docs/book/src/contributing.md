# Contributing

> This page is a stub. Contribution guidelines coming soon.

Flint is in early development. Contributions are welcome in these areas:

- **Bug reports** --- file issues on GitHub
- **Schema definitions** --- new component and archetype schemas
- **Documentation** --- improvements to this guide
- **Test coverage** --- additional unit and integration tests
- **Constraint kinds** --- new validation rule types

## Development Setup

```bash
git clone https://github.com/chaps/flint.git
cd flint
cargo build
cargo test
cargo clippy
cargo fmt --check
```

## Code Style

- Run `cargo fmt` before committing
- Run `cargo clippy` and address warnings
- Each crate has its own error type using `thiserror`
- Tests live alongside the code they test (`#[cfg(test)]` modules)
- Prefer explicit over clever; readability over brevity
