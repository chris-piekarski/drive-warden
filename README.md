# drive-warden

A Rust CLI to organize, audit, and clean up Google Drive without the web UI.

**Status:** live Google Drive backend supported for `My Drive`; Shared Drives remain deferred.

## Documentation

| Document | Description |
|----------|-------------|
| [Master plan](docs/plans/00-master-plan.md) | Architecture, CLI spec, data model, testing, Makefile |
| [Roadmap phases](docs/plans/01-roadmap-phases.md) | Implementation phases and deferred follow-on scope |
| [Open questions](docs/plans/02-open-questions.md) | Decision log and review history |
| [Getting started](docs/operator/getting-started.md) | Live and mock operator workflows |
| [Google Cloud setup](docs/operator/google-cloud-setup.md) | Desktop OAuth client setup and live path configuration |

## Quick start (live Google backend)

1. Create a Desktop OAuth client and download `credentials.json` as described in [`docs/operator/google-cloud-setup.md`](docs/operator/google-cloud-setup.md).
2. Place it at `data/credentials.json` or point the CLI at a custom path with config or `DRIVE_WARDEN_CREDENTIALS`.
3. Run the live flow:

```bash
make build
./target/debug/drive-warden auth login
./target/debug/drive-warden sync
./target/debug/drive-warden report all -o reports/live-run
./target/debug/drive-warden inspect exif <image-file-id>
./target/debug/drive-warden unshare --shared-with anyone
./target/debug/drive-warden unshare --shared-with anyone --retain-copy --apply --yes
./target/debug/drive-warden trash --path '[orphan]/Coors/Model/*'
./target/debug/drive-warden trash --path '[orphan]/Coors/Model/*' --recursive --apply --yes
./target/debug/drive-warden move --path '[orphan]/eBooks/*' --to-path '/Archive/eBooks'
./target/debug/drive-warden move --file-id <file-id> --to-folder-id <folder-id> --apply --yes
./target/debug/drive-warden trash-status --within-days 7
./target/debug/drive-warden trash-history --only-pending
./target/debug/drive-warden trash-restore --path-contains '[orphan]/Coors/Model'
./target/debug/drive-warden doctor
make gdrive-sync
./target/debug/drive-warden db remote release --name coors-trash-v1 --yes
./target/debug/drive-warden db remote release list
```

`trash --apply`, `unshare --apply`, and `move --apply` first create a named remote DB release such as `before-trash-...`, `before-unshare-...`, or `before-move-...`. If that release cannot be created, the live Drive mutation is refused. `trash --apply` moves selected actionable items to Google Drive trash only. It does not permanently delete files; restore through Google Drive during the recovery window if needed.

`make gdrive-sync` syncs the configured SQLite DB with a private visible My Drive folder via `db remote sync`. It pushes when only the local DB exists, pulls when only the remote DB exists, and fails safely when both exist so you can choose `db remote push --yes` or `db remote pull --yes`. The default remote folder is `drive-warden-db`; `db remote rename-folder --yes` migrates the legacy folder name in place. The remote folder and files must not be shared; any shared permission triggers a `SECURITY ALERT` and aborts.

`move` is preview-only by default, moves files or folders by changing their Drive parent, and supports `--to-root`, existing destinations by `--to-folder-id` or exact synced `--to-path`, and `--provision-missing` to create destination paths during apply. Use `move-history`, `trash-status`, `trash-history`, and `trash-restore` to review recoverability deadlines and manual restore guidance from the append-only trash history. Use `doctor` for a combined operator health check. Use `db remote release --name <tag> --yes` to create a named, non-overwriting DB snapshot beside the rolling remote DB copy, and `db remote release list` to discover releases.

## Quick start (mock backend)

```bash
make build
./target/debug/drive-warden --backend mock --config tests/config/mock.toml auth login
./target/debug/drive-warden --backend mock --config tests/config/mock.toml sync
./target/debug/drive-warden --backend mock --config tests/config/mock.toml report all -o reports/mock-run
make test-all
```

## Current state

The repository now ships both:

- a live `My Drive` backend with installed-app OAuth, SQLite sync, reports, `find`, `inspect`, `inspect exif`, guarded `unshare --apply`, recoverable `trash --apply`, guarded parent-change `move --apply`, remote SQLite push/pull, and optional retained-copy backup before unshare
- the original mock backend, which remains the primary offline regression gate for CI, coverage, packaging, and acceptance flows
- operator docs and recovery runbooks for live credentials, revoked tokens, invalid page tokens, scope upgrades, and sharing audits
- release packaging, shell completions, and GitHub Actions CI via the `Makefile`

## Scope

- supported: `My Drive`
- deferred: Shared Drives, permanent delete/empty-trash workflows, multi-account profiles, keyring-backed token storage

## License

TBD
