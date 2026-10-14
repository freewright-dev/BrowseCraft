# Contributing to Prowl

Thanks for helping build Prowl! Every change, however small, goes through a pull request.

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
4. Commit using [Conventional Commits](https://www.conventionalcommits.org/), e.g. `feat: support css outline`, `fix: crash on empty <table>`.
5. Push and open a pull request:
   ```sh
   git push -u origin feat/short-description
   gh pr create --fill
   ```
6. A teammate reviews it. Once it has one approval and CI is green, squash-merge it. The branch is deleted automatically.

## Tips

- Keep pull requests small and focused on one thing; they're easier to review.
- If you change how the engine works, update `docs/architecture.md`.
- Not sure where to start? Look for issues labelled `good first issue`.
