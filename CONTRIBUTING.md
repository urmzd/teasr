# Contributing

## Prerequisites

- Rust (stable toolchain)
- Chrome or Chromium (for web capture tests)
- A `GH_TOKEN` with repo access (for releases)

## Getting Started

```bash
git clone https://github.com/urmzd/teasr.git
cd teasr
cargo build --workspace
```

## Development

```bash
cargo build --workspace                          # compile all crates
cargo test --workspace                           # run tests
cargo fmt --all                                  # format code
cargo clippy --workspace -- -D warnings          # lint
cargo run -p teasr-cli -- --verbose              # run the CLI locally
cargo insta review                               # update snapshot tests
```

## Commit Convention

We use [Angular Conventional Commits](https://www.conventionalcommits.org/):

```
type(scope): description
```

Types: `feat`, `fix`, `docs`, `refactor`, `test`, `chore`, `ci`, `perf`

Commits are enforced via [gitit](https://github.com/urmzd/gitit).

## Pull Requests

1. Fork the repository
2. Create a feature branch (`feat/my-feature`)
3. Make changes and commit using conventional commits
4. Run `cargo clippy --workspace -- -D warnings && cargo test --workspace`
5. Open a pull request against `main`

## Code Style

- `cargo fmt` for formatting
- `clippy` for linting (zero warnings enforced in CI)
- No `unsafe` code
- Use `anyhow` for error handling (no custom error types)
- Use `tracing` for logging (not `println!` or `log`)
- Shared dependencies go in workspace `Cargo.toml` under `[workspace.dependencies]`
