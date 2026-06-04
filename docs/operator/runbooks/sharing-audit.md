# Sharing Audit

Use this runbook when you need to review public links, domain-wide sharing, and direct shares in a repeatable, auditable way.

## Goal

Identify exposed files, separate actionable permissions from inherited/non-actionable ones, and apply narrowly scoped `unshare` changes only after preview.

## Audit flow

1. Refresh the snapshot.

```bash
cargo run -p drive-warden -- sync
```

2. List all shared findings.

```bash
cargo run -p drive-warden -- find shared --shared
```

3. Narrow the review to a specific audience type.

```bash
cargo run -p drive-warden -- find shared --shared-with anyone

cargo run -p drive-warden -- find shared --shared-with domain:example.com
```

4. Preview the remediation plan.

```bash
cargo run -p drive-warden -- unshare --shared-with anyone
```

The preview distinguishes:

- `reason=actionable` — direct grant on the file; removed on the file itself.
- `reason=actionable_via_folder` — the grant lives on an ancestor folder you manage and is inherited here. Apply deletes it once at that source folder and the removal cascades to every inherited child.
- `reason=grantee_owned_parent` — the item is inside a folder the grantee owns, so their access is inherited from *their* folder and cannot be revoked by deleting a permission. Move the item into a folder you own to cut access.
- `reason=inherited_permission` — inherited, with no operator-managed source folder selected by the query (Shared Drive inheritance, or the source folder was filtered out). Target the source folder to revoke.
- `reason=not_actionable`
- `reason=not_owned_or_manageable`

Actionable (`actionable` and `actionable_via_folder`) rows are removed during apply.

Folder inheritance is detected from the parent chain, not the per-permission `inherited` flag — Google does not populate that flag for My Drive, so a folder grant otherwise looks like a direct grant on each child and would fail with a 403 if deleted file-by-file. The tool resolves the source folder instead.

For the live backend, an `actionable` (direct) row means all of the following are true:

- the permission is direct, not inherited
- the current operator can manage sharing on the file
- the permission is not an owner/organizer grant
- the permission type is one of `user`, `group`, `domain`, or `anyone`

## Investigate before apply

Use `inspect file <id>` for any row you are uncertain about:

```bash
cargo run -p drive-warden -- inspect file <file-id>
```

Questions to answer before applying:

- Is the permission direct or inherited?
- Does the operator own or manage the file?
- Is the target audience correct?
- Does the file need to remain shared internally while removing only public/external access?

## Apply and verify

Apply only after a successful dry run:

```bash
cargo run -p drive-warden -- unshare --shared-with anyone --apply --yes
```

If you need to retain your own backup copy before tightening sharing, use:

```bash
cargo run -p drive-warden -- unshare --shared-with anyone --retain-copy --apply --yes
```

That workflow creates a new retained-copy folder in `My Drive` first, copies the targeted file or folder tree into it, and only then removes the targeted sharing permission. If the backup step fails, the permission-removal step is not attempted for that run.

Then verify:

```bash
cargo run -p drive-warden -- find shared --shared-with anyone

cargo run -p drive-warden -- report sharing -o reports/sharing-audit
```

## Audit trail

Every applied permission deletion is written to two append-only tables:

- `audit_log` — one `delete_permission` row per affected file (timestamp, file id, permission id, target label; `source_file_id` is set to the source folder for cascade removals).
- `revoked_share_history` — a richer snapshot per affected file: `grantee`, `grantee_type`, `role`, `permission_id`, `inherited`, `source_folder_id`, `revoked_via`, and the file name/path at revoke time.

Both survive `sync` (which only rewrites `files`, `parents`, and `path_cache`), so they remain the durable record of *who lost access to what, and when* even after the live permissions are gone. Query, for example:

```sql
SELECT revoked_at, grantee, role, file_path FROM revoked_share_history ORDER BY revoked_at;
```

Use `db stats` to verify the local cache is present and healthy after the run; use SQLite tooling when you need to inspect individual rows in depth.

## Scope upgrade note

`unshare --apply` requires `drive` scope. If the current session only has read-only scopes, the CLI starts an incremental consent flow. If you decline that prompt, no live permissions are changed and the narrower session remains on disk.
