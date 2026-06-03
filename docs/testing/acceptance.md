# Acceptance

## Purpose

Acceptance is split into two layers:

- unattended offline acceptance on standalone mock data with no network or Google credentials
- a small manual live smoke checklist for a controlled `My Drive` account

## Required offline flows

1. mock login -> bootstrap sync -> report -> `inspect file` -> `inspect exif` -> logout
2. sync -> storage analysis -> large-file discovery
3. sync -> sharing audit -> `unshare` preview -> `unshare` apply -> verify
4. revoked token -> re-login guidance
5. invalid page token -> `sync --full` guidance
6. interrupted sync -> rerun -> preserved committed snapshot
7. non-interactive automation with stable JSON/text output
8. configured live Google paths fail cleanly without secrets

## Backing suites

- `crates/gdrive-optimize/tests/acceptance_mock_end_to_end.rs`
- focused functional suites under `crates/gdrive-optimize/tests/cli_*_functional.rs`
- fixture validation via `crates/gdrive-optimize/tests/fixtures_validate.rs`

## Manual live layer

Run [`live-smoke.md`](./live-smoke.md) only after the offline gate is green.

## Gate

- unattended gate: `make test-all`
- release hardening gate: `make test-coverage`, `make build-release`, package verification, and the manual live smoke checklist
