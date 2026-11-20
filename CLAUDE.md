# radar30

@docs/pitfalls.md

What works today and how it was checked: `docs/STATUS.md`. Choices and their reasons:
`DECISIONS.md`. Multi-wave roadmap: `docs/PLAN.md`.

Before finishing any change:
1. `cargo fmt --check`
2. `cargo clippy --all-targets -- -D warnings`
3. `cargo test`
4. `scripts/check-secrets.sh --all`
5. Update `docs/STATUS.md` when verified capabilities change.
