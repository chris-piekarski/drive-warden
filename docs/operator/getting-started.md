# Getting Started

**Drive Warden** is a local-first CLI for auditing and supervising Google Drive metadata from an intake ledger (SQLite snapshot). The warden runs security briefings, enforces clearance rules, and applies guarded remediation—without the web UI. The live Google backend supports `My Drive` end to end; the fixture-backed mock backend remains the offline verification path for CI and safe experimentation.

## Supported workflows

- live Google OAuth login/logout/status for a Desktop OAuth client
- bootstrap and delta sync into SQLite (roll call)
- Markdown warden briefings (`report all`, `report summary`, etc.) and terminal triage (`report attention`)
- `find`, `inspect file`, and `inspect exif`
- `backup shared-with-me` local archive with resumable `manifest.jsonl`
- `shared declutter` preview and guarded apply (remove backed-up shared-with-you files from My Drive)
- `unshare` preview plus guarded `unshare --apply`
- `trash` preview plus guarded, recoverable `trash --apply`
- `move` preview plus guarded parent changes into existing folders
- warden rounds (`doctor`) and exact-duplicate review guidance ([duplicate cleanup](duplicate-cleanup.md))
- local database inspection with `db stats` and `db vacuum`
- remote SQLite push/pull with `db remote` and `make gdrive-sync`

## Scope limits

- supported: `My Drive`
- deferred: Shared Drives, permanent delete/empty-trash workflows, multi-account profiles

## Prerequisites

- Rust toolchain matching [`rust-toolchain.toml`](../../rust-toolchain.toml)
- `cargo`
- a writable local workspace for the SQLite database, OAuth token/session files, and generated reports
- a Desktop OAuth client `credentials.json` file for the Google backend

## Live quick start

Build the CLI:

```bash
make build
```

If you want to use the default local layout, place your Google Desktop OAuth client file at `data/credentials.json`. Then:

```bash
cargo run -p drive-warden -- auth login
cargo run -p drive-warden -- sync
cargo run -p drive-warden -- report all -o reports/live-run
cargo run -p drive-warden -- report attention --manifest backups/shared-with-me/manifest.jsonl
```

Inspect a specific file or image:

```bash
cargo run -p drive-warden -- inspect file <file-id>
cargo run -p drive-warden -- inspect exif <image-file-id>
```

Preview and apply sharing cleanup:

```bash
cargo run -p drive-warden -- unshare --shared-with anyone
cargo run -p drive-warden -- unshare --shared-with anyone --apply --yes
cargo run -p drive-warden -- unshare --shared-with anyone --retain-copy --apply --yes
```

The CLI performs a follow-up full sync after a successful `unshare --apply` so the local snapshot reflects the new permission state immediately.

Back up shared-with-me content before removing it from your My Drive:

```bash
cargo run -p drive-warden -- backup shared-with-me --out backups/shared-with-me
cargo run -p drive-warden -- shared declutter --manifest backups/shared-with-me/manifest.jsonl
cargo run -p drive-warden -- shared declutter --manifest backups/shared-with-me/manifest.jsonl --apply --yes
```

`backup shared-with-me` writes files under `--out` (default `backups/shared-with-me`) and appends one JSON object per line to `manifest.jsonl` in that directory unless `--manifest` points elsewhere. Re-runs skip rows already marked successful (`downloaded`, `exported`, `copied`, `recovered_export`, or `folder` placeholder directories). Use `--reuse-manifest` to copy bytes from a prior JSON manifest when filenames match.

`shared declutter` is preview-only by default. Apply removes only **files** that have a successful backup manifest row from your My Drive (via `removeParents=root`). It does not trash the owner's copy, does not remove folder placeholders, and skips items missing from the manifest or marked unresolved (for example Google Earth projects). When actionable rows exist, `--apply --yes` creates a `before-shared-declutter-...` remote DB release and runs a full sync afterward.

Use `--retain-copy` when you need the CLI to create a private backup copy in `My Drive` before removing the targeted sharing permission. The default destination is a new retained-copy folder under `My Drive`; use `--backup-root-id <folder-id>` to place the auto-created run folder under a specific existing folder instead.

Preview and apply recoverable trash cleanup:

```bash
cargo run -p drive-warden -- trash --path '[orphan]/Coors/Model/*'
cargo run -p drive-warden -- trash --path '[orphan]/Coors/Model/*' --recursive --apply --yes
cargo run -p drive-warden -- trash-status --within-days 7
cargo run -p drive-warden -- trash-history --only-pending
cargo run -p drive-warden -- trash-restore --path-contains '[orphan]/Coors/Model'
cargo run -p drive-warden -- move --path '[orphan]/eBooks/*' --to-path '/Archive/eBooks'
cargo run -p drive-warden -- move --file-id <file-id> --to-root --apply --yes
cargo run -p drive-warden -- move --path '/Docs/*' --to-path '/Archive/New' --provision-missing --apply --yes
cargo run -p drive-warden -- move-history --only-pending
cargo run -p drive-warden -- doctor
```

`trash --apply`, `unshare --apply`, and `move --apply` first create a named remote DB release such as `before-trash-...`, `before-unshare-...`, or `before-move-...`. If that release cannot be created, the live Drive mutation is refused. `trash --apply` moves files or explicitly recursive folders to Google Drive trash. It does not permanently delete files or empty trash; use the Google Drive UI if you need to restore items during Google's recovery window.

`move` supports My Drive root (`--to-root`), existing destinations by exact synced path or folder ID, and `--provision-missing` to create missing destination folders during apply. It records pending and applied rows in `moved_file_history`, then runs a full sync so paths reflect the new parent.

Every applied trash move is recorded in the append-only `trashed_file_history` table. Recursive folder trash records descendant snapshots too, so operators can still see file IDs, paths, trash time, and estimated recovery deadlines after sync removes those items from the active `files` inventory.

`trash-status` summarizes pending and estimated-expired segregation history rows and warns when recoverability expires within the requested window. `trash-history` lists individual rows without requiring raw SQL. `trash-restore` is read-only and prints manual Google Drive restore guidance for matching history rows. `doctor` (warden rounds) combines intake ledger stats, facility quota, remote DB state, release count, and segregation deadline warnings in one check.

For exact-MD5 duplicate groups, follow the review-first workflow in [duplicate-cleanup.md](duplicate-cleanup.md) before any `trash --apply`.

## Configuring live paths

The live backend accepts first-class config for credentials, token cache, and session metadata:

```toml
[backend]
kind = "google"

[database]
path = "data/inventory.db"
remote_folder_name = "drive-warden-db"
remote_db_name = "inventory.db"
remote_manifest_name = "inventory.db.manifest.json"

[google]
credentials_path = "data/credentials.json"
token_path = "data/google-tokens.json"
session_path = "data/google-session.json"

[reports]
output_dir = "reports"
stale_threshold_days = 730
```

Environment overrides:

- `DRIVE_WARDEN_CREDENTIALS`
- `DRIVE_WARDEN_TOKENS`
- `DRIVE_WARDEN_GOOGLE_SESSION`

Using `--db /path/to/inventory.db` is still the easiest way to isolate one local profile from another. By default, live session and token files live beside the selected database path.

## Remote DB Sync

Use remote DB sync when you want the SQLite inventory available across systems:

```bash
make gdrive-sync
cargo run -p drive-warden -- db remote status
cargo run -p drive-warden -- db remote push --yes
cargo run -p drive-warden -- db remote pull --yes
cargo run -p drive-warden -- db remote rename-folder --yes
cargo run -p drive-warden -- db remote release --name coors-trash-v1 --yes
cargo run -p drive-warden -- db remote release list
```

`db remote sync` pushes when only the local DB exists and pulls when only the remote DB exists. If both exist, it fails safely and prints timestamps/checksum context so you can choose explicit `push` or `pull`.

The remote DB lives in a visible My Drive folder. Configure `remote_folder_id` if you want to use a specific existing folder; otherwise the CLI uses `remote_folder_name` under `My Drive` root and creates it on push. The default folder is `drive-warden-db`; `db remote rename-folder --yes` renames the legacy `gdrive-optimize-db` folder in place while preserving its folder ID and release files. The folder, DB file, and manifest file must remain private. Any `anyone`, `domain`, or non-owner user permission triggers `SECURITY ALERT` and aborts before transfer.

Push creates a consistent SQLite snapshot with `VACUUM INTO`, uploads the DB, and writes a SHA-256 manifest next to it. Pull downloads to a temp file, verifies the manifest, backs up any existing local DB, and then atomically replaces the configured DB path.

Each database has a stable `db_instance_id` and remote sync generation stored in SQLite. Remote manifests include that identity and generation so `db remote status` can show whether the local DB and remote DB are tracking the same logical copy, not just similarly named files.

`db remote release --name <tag> --yes` creates named release files in the same private Drive folder, for example `inventory.coors-trash-v1.db` and `inventory.coors-trash-v1.db.manifest.json`. Releases are non-overwriting; rerunning the same tag fails instead of replacing the prior snapshot. Use `db remote release list` to see named release DB and manifest pairs.

## Scope progression

`drive-warden` keeps the live backend least-privilege by default:

- `auth login` requests `drive.metadata.readonly`
- `inspect exif` upgrades to `drive.readonly` if the current session is narrower
- `backup shared-with-me` upgrades to `drive.readonly` when export or download is required
- `unshare --apply` upgrades to `drive` if the current session is narrower
- `trash --apply` upgrades to `drive` if the current session is narrower
- `move --apply` upgrades to `drive` if the current session is narrower
- `shared declutter --apply` upgrades to `drive` if the current session is narrower
- `db remote push` upgrades to `drive`; `db remote pull` may upgrade to `drive.readonly`

If the broader consent flow is declined, the narrower session remains on disk and the command fails safely.

## Mock quick start

The mock backend is still the best way to learn the command surface offline:

```bash
cargo run -p drive-warden -- \
  --backend mock \
  --config tests/config/mock.toml \
  auth login

cargo run -p drive-warden -- \
  --backend mock \
  --config tests/config/mock.toml \
  sync

cargo run -p drive-warden -- \
  --backend mock \
  --config tests/config/mock.toml \
  report all -o reports/mock-run
```

## Output modes

- default output is a stable plain-text format for terminals (warden console voice on key commands)
- `--format json` emits machine-readable output for `find`, `inspect`, `unshare`, `trash`, `move`, `backup`, `shared declutter`, `report attention`, and `db`
- `--no-interactive` disables prompts that would block unattended automation
- `report summary|duplicates|sharing|storage|all` write Markdown warden briefings to disk; `report attention` prints to stdout only

## Shell completions

Generate completions for your shell:

```bash
cargo run -p drive-warden -- completions bash
cargo run -p drive-warden -- completions zsh
cargo run -p drive-warden -- completions fish
```

Or generate all supported completion files into `dist/completions/`:

```bash
make completions
```

## Local data layout

Default live layout:

```text
data/
├── credentials.json
├── google-session.json
├── google-tokens.json
└── inventory.db
```

Default mock layout:

- `inventory.db` wherever `--db` points
- mock auth/session state beside that database path

## Troubleshooting

- `not logged in`
  Run `auth login` for the selected backend/config before syncing or inspecting EXIF metadata.
- `invalid page token` or `410 Gone`
  Rerun `sync --full` to rebuild the local snapshot from the authoritative Drive state.
- `revoked or expired`
  Re-run `auth login` to refresh the session. See [`runbooks/revoked-token-recovery.md`](runbooks/revoked-token-recovery.md).
- `credentials were not found`
  Verify the configured `credentials_path` points to a Desktop OAuth client JSON file.
- `SECURITY ALERT: remote DB endpoint must not be shared`
  Remove sharing from the configured remote DB folder, DB file, and manifest file in Google Drive, then rerun `db remote status`.
- `inspect exif` reports no metadata
  The current implementation prefers Drive `imageMediaMetadata` only. If Google Drive has no image metadata for that file, EXIF byte-download fallback is intentionally skipped.
