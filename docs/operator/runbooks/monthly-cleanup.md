# Monthly Cleanup

Use this runbook for a repeatable monthly review of stale files, duplicate storage, and easy sharing cleanup candidates.

## Goal

Refresh the local snapshot, generate a report pack, review large/stale items, and preview any obvious trash or sharing remediations before applying them.

## Recommended sequence

1. Refresh the snapshot.

```bash
cargo run -p gdrive-optimize -- sync
```

2. Generate the full report pack.

```bash
cargo run -p gdrive-optimize -- report all -o reports/monthly-cleanup
```

3. Review oversized or stale files.

```bash
cargo run -p gdrive-optimize -- find large --min 5000000
```

4. Review duplicate groups.

```bash
cargo run -p gdrive-optimize -- find duplicates --limit 25
```

5. Preview broad sharing cleanup candidates.

```bash
cargo run -p gdrive-optimize -- unshare --shared
```

6. Inspect anything ambiguous before acting.

```bash
cargo run -p gdrive-optimize -- inspect file <file-id>
```

7. Preview recoverable trash cleanup for stale build artifacts.

```bash
cargo run -p gdrive-optimize -- trash --path '[orphan]/Coors/Model/*'
```

Use `--recursive` only after reviewing folder rows and descendant counts.

## Before applying changes

- confirm the item is directly actionable rather than inherited
- confirm the target label matches the audience you intend to remove
- prefer `--shared-with anyone` or `--shared-with domain:<name>` over unbounded mutations when possible
- use `trash --apply` only for recoverable trash moves; it never permanently deletes files
- use `--no-interactive --yes` only in scripted environments that already reviewed a dry run

## Apply examples

```bash
cargo run -p gdrive-optimize -- unshare --shared-with anyone --apply --yes
cargo run -p gdrive-optimize -- trash --path '[orphan]/Coors/Model/*' --recursive --apply --yes
```

The CLI performs a follow-up sync after a successful apply so reports and follow-up queries reflect the new state.

Before any live `trash --apply` or `unshare --apply` mutation, the CLI creates a named remote DB release. Confirm the output includes `pre-mutation release:`; if the release fails, the mutation is refused.

Trash apply writes durable rows to `trashed_file_history` before the follow-up sync removes trashed items from the active inventory. For recursive folder trash, the history table includes both explicitly requested folders/files and all descendants from the pre-trash snapshot, with an estimated `recoverable_until` timestamp.

## Verification

- rerun `find shared` with the same filter you used during preview
- rerun the same `trash` dry run and confirm moved items no longer appear in the local snapshot
- run `trash-status --within-days 7` and `trash-history --only-pending` if any trash cleanup was applied
- run `trash-restore --path-contains <path-fragment>` when an operator needs manual restore steps
- regenerate `report sharing`
- inspect `doctor` or `db stats` to confirm the local cache and remote DB state are healthy

```bash
cargo run -p gdrive-optimize -- doctor
cargo run -p gdrive-optimize -- db stats
cargo run -p gdrive-optimize -- trash-status --within-days 7
cargo run -p gdrive-optimize -- trash-history --only-pending
cargo run -p gdrive-optimize -- db remote release list
```

```sql
SELECT trashed_at, recoverable_until, file_path
FROM trashed_file_history
ORDER BY recoverable_until;
```

## Mock rehearsal

If you want to rehearse the same sequence with deterministic data and no network, rerun the commands above with:

```bash
--backend mock --config tests/config/mock.toml
```
