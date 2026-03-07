# Contributing to KubeRift

All contributions are valued — bug reports, documentation fixes, tests, and features alike.

## Code of Conduct

This project follows the [Contributor Covenant v2.1](https://www.contributor-covenant.org/version/2/1/code_of_conduct/). Be respectful and constructive.

## Getting Started

```bash
# Fork and clone
git clone https://github.com/<you>/kuberift.git
cd kuberift

# Build
cargo build

# Run all checks (do this before opening a PR)
cargo fmt -- --check
cargo clippy -- -D warnings
cargo test
```

**Requirements:** Rust 1.82+ (see `rust-version` in `Cargo.toml`). No other tooling needed.

## Finding Work

- Browse issues labeled [`good first issue`](https://github.com/syedazeez337/kuberift/labels/good%20first%20issue) or [`help wanted`](https://github.com/syedazeez337/kuberift/labels/help%20wanted)
- Comment on the issue to claim it before starting work
- If no suitable issue exists, open one first to discuss your idea

## Making Changes

### Branch naming

```
feat/<short-description>     # new features
fix/<short-description>      # bug fixes
docs/<short-description>     # documentation
```

Include the issue number when applicable: `feat/39-config-file`

### Commit messages

Use [Conventional Commits](https://www.conventionalcommits.org/):

```
feat(config): add TOML config file loading

Reads ~/.config/kuberift/config.toml on startup and merges
with CLI args. CLI flags always take precedence.

Closes #39
```

Types: `feat`, `fix`, `docs`, `test`, `refactor`, `ci`, `chore`

### Code quality gates

Every PR must pass:

1. **`cargo fmt -- --check`** — no manual formatting
2. **`cargo clippy -- -D warnings`** — zero warnings
3. **`cargo test`** — all tests pass

Do not modify `rustfmt.toml` or clippy configuration without prior discussion.

### Tests

- Add tests for new functionality. Place unit tests in `#[cfg(test)] mod tests` within the source file, integration tests in `tests/`.
- When fixing a bug, add a test that would have caught it.
- Run `cargo test` locally before pushing.

### What makes a good PR

- **One concern per PR.** A bug fix and a new feature should be separate PRs.
- **Link the issue.** Use `Closes #N` or `Fixes #N` in the PR body.
- **Describe what and why.** The diff shows *what* changed; the description should explain *why*.
- **Keep it small.** Smaller PRs get reviewed faster and merged sooner.

## PR Template

When opening a PR, include:

```markdown
## Summary
<What changed and why — 1-3 bullets>

## Test plan
- [ ] `cargo test` passes
- [ ] `cargo clippy -- -D warnings` clean
- [ ] <manual verification steps if applicable>

Closes #<issue>
```

## Review Process

- Any community member can review and leave feedback.
- At least one maintainer approval is required to merge.
- We aim to provide initial feedback within 48 hours.
- Nits and style suggestions won't block merging — we can address those in follow-ups.
- If a PR goes stale, we'll check in. If you're stuck, ask for help.

## Project Structure

```
src/
├── main.rs          # Entry point, skim TUI loop, action dispatch
├── cli.rs           # clap argument parsing
├── config.rs        # Config file loading (~/.config/kuberift/config.toml)
├── items.rs         # K8sItem, ResourceKind, StatusHealth, SkimItem impl
├── actions.rs       # kubectl action handlers (describe, logs, exec, delete, etc.)
├── lib.rs           # Re-exports for tests
└── k8s/
    ├── client.rs    # kube::Client builder, context management
    └── resources.rs # watch_resources(), resource type watchers
tests/               # Integration tests
```

## MSRV Policy

The minimum supported Rust version is declared in `Cargo.toml` (`rust-version`). MSRV bumps require a minor version bump and should be discussed in an issue first.

## License

By contributing, you agree that your contributions are licensed under the [MIT License](LICENSE).
