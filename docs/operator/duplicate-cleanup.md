# Exact Duplicate Cleanup

Exact duplicate cleanup is a deliberate operator workflow. `drive-warden` can help find and verify duplicate groups, but it must not trash or delete files unless the operator explicitly selects the target file IDs and confirms an apply command.

## Policy

- Treat exact-MD5 matches as cleanup candidates, not automatic deletions.
- Choose one keeper per group before applying any mutation.
- Prefer exact `--file-id` targeting for cleanup batches.
- Avoid broad filters such as `--duplicate-of` on `trash` unless the preview has been reviewed line by line.
- Use preview first, then `--apply --yes` only after confirming the file IDs are the intended duplicate copies.
- Keep remote DB pre-mutation releases enabled for recoverability and audit context (`before-trash-...` releases are created automatically on apply).

## Review Workflow

Start with the duplicate finder or warden briefing:

```bash
drive-warden find duplicates --format table
drive-warden report duplicates -o reports/review
drive-warden report attention
```

For each exact-MD5 group:

1. Inspect all cell paths, ownership, modified times, and clearance state (`inspect file <id>`).
2. Pick the keeper based on folder location, ownership, metadata quality, and whether the copy is shared externally.
3. Copy the duplicate inmate IDs into a small reviewed batch.
4. Preview segregation by exact file ID:

```bash
drive-warden trash --file-id <duplicate-file-id>
```

Apply only after the preview shows exactly the intended item:

```bash
drive-warden trash --file-id <duplicate-file-id> --apply --yes
```

For multiple files, run one reviewed command per file or use a carefully reviewed shell loop that prints each file ID before invoking `drive-warden trash`. Do not build cleanup batches from unreviewed broad filters.

`trash --apply` only applies to files **owned by the authenticated operator** (`owned_by_me=true`). Shared-with-you duplicates cannot be trashed this way.

## Shared-with-me copies

For shared-with-you duplicates (`owned_by_me=false`), back up first, then declutter from your My Drive:

```bash
drive-warden backup shared-with-me --out backups/shared-with-me
drive-warden shared declutter --manifest backups/shared-with-me/manifest.jsonl
drive-warden shared declutter --manifest backups/shared-with-me/manifest.jsonl --apply --yes
```

Notes:

- Backup writes `manifest.jsonl` under `--out` by default (override with `backup --manifest`). `shared declutter` requires `--manifest` explicitly — there is no default on that command.
- `shared declutter` removes the item from **your** My Drive (`removeParents=root`). It does not trash the owner's original file.
- Folder manifest rows are placeholders only; declutter skips them. Verify descendant file coverage before expecting a folder tree to disappear from My Drive.
- Unresolved backup rows (for example Google Earth projects) are not decluttered until exported manually.

After declutter apply, the CLI creates a remote DB release when needed and runs a full sync so the intake ledger matches Drive.

## Related commands

| Goal | Command |
|------|---------|
| Triage duplicates + backup gaps + releases | `report attention --manifest …` |
| Markdown duplicate briefing | `report duplicates -o reports/review` |
| Warden rounds health check | `doctor` |
