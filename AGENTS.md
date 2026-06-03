# AGENTS.md

This file helps coding agents get productive in `gdrive-optimize` without rereading the entire repo. Keep it current when architecture, commands, or safety rules change.

## Project Summary

`gdrive-optimize` is a Rust CLI for syncing Google Drive metadata into a local SQLite snapshot, auditing duplicates/sharing/storage, inspecting file metadata, safely applying guarded sharing or recoverable trash cleanup, and syncing the SQLite DB to a private visible My Drive folder. The live backend supports `My Drive`; Shared Drives, permanent delete/empty-trash workflows, multi-account profiles, and keyring storage are deferred.

The project is local-first and safety-first:

- Reports and find commands read only the last committed SQLite snapshot.
- Live Google writes are behind explicit `unshare --apply` or `trash --apply` plus confirmation or `--yes`.
- Mock fixtures are the primary offline regression surface.
- Runtime data belongs under `data/`, `reports/`, `target/`, or temp dirs, not in source logic.

## Workspace Layout

- `crates/gdrive-core/`: domain models, traits/ports, sync engine, query/filter/report data shaping, unshare/retain-copy planning and apply orchestration.
- `crates/gdrive-drive/`: `DriveGateway` adapters. Contains live Google Drive API/OAuth code and the fixture-backed mutable mock gateway.
- `crates/gdrive-db/`: `InventoryRepository` SQLite adapter, embedded `refinery` migrations, path cache persistence, audit log, revoked-share history, DB stats/vacuum.
- `crates/gdrive-report/`: Markdown report rendering and file writer.
- `crates/gdrive-optimize/`: CLI composition root using `clap`; parses config/env/flags, builds adapters, calls core use cases, prints table/JSON output.
- `migrations/`: SQLite migrations embedded by `gdrive-db`.
- `tests/fixtures/`: mock Drive API datasets. `drive_small` is the default happy path; `drive_failures/*` cover recovery scenarios.
- `tests/config/mock.toml`: default mock backend config.
- `docs/`: design, architecture, testing, and operator docs. Update docs when behavior changes.
- `reports/live-run/`: checked-in generated report samples. Avoid changing them unless fixing stale generated content or intentionally refreshing reports.

Ignore build/coverage artifacts such as `target/`, `lcov.info`, and generated release/completion outputs unless the task explicitly involves them.

## Architecture Rules

Dependencies point inward:

```text
gdrive-optimize binary
  -> gdrive-core
  -> gdrive-drive
  -> gdrive-db
  -> gdrive-report

adapter crates -> gdrive-core only
```

The binary crate is the only composition root. Adapter crates should not depend on each other. Put reusable behavior and contracts in `gdrive-core`; keep Google API details, SQLite details, and Markdown rendering in their adapter crates.

Important core ports:

- `DriveGateway`: auth, file listing, change listing, EXIF inspection, scope upgrades, folder/copy/permission writes.
- `InventoryRepository`: committed snapshot state, sync run journaling, snapshot replacement, audit log, path cache inspection.
- `ReportWriter`: Markdown output sink.

## Behavioral Invariants

- `sync_inventory()` must use account-specific committed page tokens. A different authenticated account should force a full sync instead of consuming another account's token.
- Full sync flow: get start page token, collect `files.list` pages, apply changes since the checkpoint, then commit snapshot/token atomically.
- Delta sync flow: load committed snapshot, replay `changes.list` pages from the committed token, then commit snapshot/token atomically.
- If sync fails after a run starts, mark the run failed and preserve the previously committed snapshot.
- Bootstrap list excludes trash (`trashed = false`); delta changes for trashed/removed files remove them from the local snapshot.
- Path cache is derived from parents and is rebuildable. Preserve explicit `resolved`, `multi_parent`, and `orphaned` states.
- Query filters should filter before result expansion/sorting, and apply `limit`/`offset` only after the final ordered result set is built.
- Live Google Drive requests must go through the shared retry/timeout helper; do not add raw `.doit().await`/`.upload().await` call sites without the wrapper.
- Delta sync must auto-fall back to full sync when Google reports an expired/invalid changes page token.
- `unshare` without `--apply` is preview-only. `--dry-run` is an explicit preview alias and must not be combined with `--apply`.
- `unshare --apply` must create a named remote DB release before any live mutation; if release creation fails, the permission delete must not run. It removes only actionable rows (`actionable` or `actionable_via_folder`) and records a pending audit row before each live permission delete plus final records in both `audit_log` and `revoked_share_history`. Folder-inherited grants are detected from the parent chain — Google omits the per-permission `inherited` flag for My Drive, so the synced flag is not trusted on its own — and are revoked by a single cascade delete at the operator-managed source folder, with each affected child recorded individually. Rows inherited from a folder the grantee owns are `grantee_owned_parent` (unrevokable until the item is moved out); `inherited_permission`, `not_actionable`, and `not_owned_or_manageable` rows remain visible in preview but are skipped.
- Any permission removal — including out-of-band Drive API deletes for cases the tool cannot yet handle — must be recorded in `audit_log` and `revoked_share_history`. `sync` overwrites `files.permissions_json` with current state, so those two append-only tables are the only durable record of revoked access.
- `trash` without `--apply` is preview-only. `trash --apply` must create a named remote DB release before any live mutation; if release creation fails, the trash call must not run. It moves actionable rows to Google Drive trash, never permanently deletes, requires a committed local sync snapshot, writes a pending audit row before the live trash call, and requires `--recursive` before folder rows become actionable.
- `trash-status` and `trash-history` read append-only `trashed_file_history`; status must warn on recoverability deadlines inside the configured window.
- `trash-restore` is read-only guidance only; do not imply Drive restore mutation exists unless it is actually implemented and audited.
- `doctor` is the read-only operator health check and should remain safe to run before destructive or remote decisions.
- `db remote sync` pushes only when local DB exists and remote DB is missing, pulls only when local DB is missing and remote DB exists, and fails when both exist until the operator chooses explicit `db remote push --yes` or `db remote pull --yes`.
- `db remote release --name <tag> --yes` creates named DB snapshot files and must never overwrite an existing release tag. `db remote release list` should discover release DB/manifest pairs without mutating Drive.
- Remote DB folders/files must be private and owned by the authenticated operator. Any `anyone`, `domain`, non-owner user permission, or `ownedByMe=false` endpoint is a `SECURITY ALERT` and must abort before transfer.
- Remote DB push uses a consistent SQLite snapshot plus SHA-256 manifest. Pull verifies the manifest, writes a temp file, backs up the current local DB, then atomically replaces it.
- SQLite migrations include `db_identity` and `remote_sync_state`. Keep remote manifests aligned with `db_instance_id` and sync generation so cross-machine conflict messages stay meaningful.
- `--retain-copy` creates private backup folders/files before removing targeted permissions and writes audit log entries for backup and permission actions.
- `inspect exif` currently reads Drive `imageMediaMetadata`; byte-download EXIF fallback is intentionally not implemented.

## CLI And Config Notes

Default config path is `data/config.toml`; missing config is allowed. Useful knobs:

- Global flags: `--backend google|mock`, `--config <path>`, `--db <path>`, `--format table|json`, `--no-interactive`.
- Live env overrides: `GDRIVE_OPTIMIZE_CREDENTIALS`, `GDRIVE_OPTIMIZE_TOKENS`, `GDRIVE_OPTIMIZE_GOOGLE_SESSION`.
- Default DB: `data/inventory.db`.
- Default live credentials/session/token paths live beside the selected DB unless configured.
- Default report directory is configured `reports/<date>/` unless `-o <dir>` is passed.
- Default stale threshold is 730 days.

Mock quick flow:

```bash
cargo run -p gdrive-optimize -- --backend mock --config tests/config/mock.toml auth login
cargo run -p gdrive-optimize -- --backend mock --config tests/config/mock.toml sync
cargo run -p gdrive-optimize -- --backend mock --config tests/config/mock.toml report all -o reports/mock-run
```

## Testing Commands

Prefer the narrowest useful test first, then broader suites for shared behavior.

```bash
cargo fmt --all
cargo test --workspace
make lint
make test-all
```

Targeted suites:

```bash
cargo test -p gdrive-core --lib
cargo test -p gdrive-core --test sync_integration
cargo test -p gdrive-db --test db_integration
cargo test -p gdrive-db --test path_cache_integration
cargo test -p gdrive-optimize --test cli_sync_functional
cargo test -p gdrive-optimize --test cli_report_functional
cargo test -p gdrive-optimize --test cli_find_functional
cargo test -p gdrive-optimize --test cli_polish_functional
cargo test -p gdrive-optimize --test cli_unshare_functional
cargo test -p gdrive-optimize --test acceptance_mock_end_to_end
```

Coverage target:

```bash
make test-coverage
```

The Makefile enforces formatting and Clippy via `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets -- -D warnings`.

## Where To Add Tests

- Pure domain/query/sync helper logic: `crates/gdrive-core/src/lib_tests.rs`.
- End-to-end core orchestration with fake gateways/repos: `crates/gdrive-core/tests/sync_integration.rs`.
- SQLite persistence, migrations, path cache: `crates/gdrive-db/tests/*` or `crates/gdrive-db/src/lib_tests.rs`.
- Google/mock adapter mapping, scope/session behavior, fixture mutation: `crates/gdrive-drive/src/lib_tests.rs`.
- CLI behavior through the compiled binary: `crates/gdrive-optimize/tests/*`.
- Markdown rendering/writer details: `crates/gdrive-report/src/lib_tests.rs`.
- Fixture schema/existence checks: `crates/gdrive-optimize/tests/fixtures_validate.rs`.

Functional and acceptance tests must use mock fixtures, not live Google.

## Coding Conventions

- Keep Rust formatted with `cargo fmt --all`.
- Use workspace dependency versions in root `Cargo.toml` where possible.
- Use `CoreResult<T>` and `CoreError` in library crates; use `anyhow::Result` only in the binary.
- Preserve trait boundaries. Do not let `gdrive-core` learn about SQLite, Google SDK types, filesystem config, or CLI parsing.
- Prefer structured data and serde/rusqlite APIs over ad hoc string parsing.
- Keep output stable for tests and automation, especially `--format json` paths.
- Be conservative with live Google behavior: least-privilege scopes, no Shared Drives unless implementing that deferred feature, no permanent delete/empty-trash operations unless explicitly added, no content downloads unless the feature explicitly adds them.
- Do not commit credentials, OAuth tokens, local DBs, or other secrets. `credentials.json`, token/session files, and local databases should stay out of source changes.

## Common Change Recipes

- Add a CLI flag: update `QueryFilters`/args in `crates/gdrive-optimize/src/main.rs`, map it into `InventoryQuery` or command options, add CLI functional coverage, and update docs/help examples if user-visible.
- Add a Drive metadata field: update `FileRecord`, Google field masks and mapping in `gdrive-drive`, mock fixture shape as needed, SQLite migration and load/save code, reports/tests/docs.
- Change sync behavior: update `gdrive-core` first, then SQLite repository behavior if persistence semantics change; add fake-gateway integration coverage and failure-path tests.
- Change reports: update `gdrive-report`, report functional tests, and any checked-in generated report samples if they are intentionally kept current.
- Change unshare/write behavior: update plan/apply code in `gdrive-core`, CLI guardrails in `gdrive-optimize`, mock mutation behavior if needed (folder-grant deletes must cascade to inherited descendants), `audit_log` and `revoked_share_history` expectations, and non-interactive tests.
- Change trash/write behavior: update `TrashPlan`/`apply_trash` in `gdrive-core`, live and mock `DriveGateway::trash_file`, CLI guardrails, audit expectations, and post-apply sync tests.
- Change remote DB sync behavior: update core manifest/privacy models, live and mock remote file operations, CLI `db remote` guardrails, Make targets, manifest verification tests, and docs.

## Documentation Pointers

- `README.md`: operator quick start and current scope.
- `docs/operator/getting-started.md`: live/mock workflows and troubleshooting.
- `docs/operator/google-cloud-setup.md`: OAuth client setup.
- `docs/operator/runbooks/`: recovery and operational procedures.
- `docs/architecture/overview.md`: crate boundaries.
- `docs/architecture/sync-engine.md`: sync invariants.
- `docs/architecture/path-model.md`: path/orphan/multi-parent model.
- `docs/design/filter-flags.md`: filter grammar.
- `docs/testing/strategy.md` and `docs/testing/fixtures.md`: test structure and fixture contract.
