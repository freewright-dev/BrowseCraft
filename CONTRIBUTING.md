# Contributing to BrowseCraft

Thanks for helping build BrowseCraft! Every change, however small, goes through a pull request.

## Workflow

1. Start from an up-to-date `main`:
   ```sh
   git checkout main
   git pull
   ```
2. Create a branch for your change:
   ```sh
   git checkout -b feat/short-description   # or fix/..., docs/..., chore/...
   ```
3. Make your change, then check it builds and the tests pass:
   ```sh
   cargo build
   cargo test
   ```
   On Windows, `build.bat test` runs the tests too. You need Rust 1.95+ with the MSVC toolchain; see [docs/development.md](docs/development.md) for the full setup.
4. Commit using [Conventional Commits](https://www.conventionalcommits.org/), e.g. `feat: support css outline`, `fix: crash on empty <table>`.
5. Push and open a pull request:
   ```sh
   git push -u origin feat/short-description
   gh pr create --fill
   ```
6. A teammate reviews it. CI runs `cargo build --locked` and `cargo test --locked` on Windows. Once it has one approval and CI is green, squash-merge it. The branch is deleted automatically.

## Tips

- Keep pull requests small and focused on one thing; they're easier to review.
- If you change how the engine works, update `docs/architecture.md`.
- If you change the build, tests or where code lives, update `docs/development.md`.
- Not sure where to start? Look for issues labelled `good first issue`.
