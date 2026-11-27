# Cullman legal index

Private reference index of law firms, practicing attorneys and public legal offices in Cullman County, Alabama, built to extend to neighbouring counties.

- Plan: "Cullman Legal Index: Build Plan" (v2, Nov 27, 2026) in the Cullman Alabama project.
- How it runs: `RUNBOOK.md`. Rules for agents: `CLAUDE.md`.
- Build tool: `lidx` (Rust). `cargo run -- validate`, `cargo run -- build`.

Canonical data is `registry/`, `decisions/` and the append-only `observations/`. Everything in `data/` and `build/` is generated. Sales data is never stored here.
