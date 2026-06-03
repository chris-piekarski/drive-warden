# Scope Upgrade Prompts

Use this runbook when a live command needs broader Google Drive permissions than the current session already has.

## Current progression

- `auth login` starts at `drive.metadata.readonly`
- `inspect exif` may upgrade to `drive.readonly`
- `unshare --apply` may upgrade to `drive`
- `trash --apply` may upgrade to `drive`
- `db remote pull` may upgrade to `drive.readonly`
- `db remote push` may upgrade to `drive`

## What happens

1. The command checks the stored live session scopes.
2. If the current session is too narrow, the CLI starts an incremental OAuth consent flow.
3. If consent succeeds, the broader scope set replaces the stored live session.
4. If consent is declined or the flow fails, the prior narrower session remains on disk.

## Safe operator pattern

1. Start with a fresh read-only session.

```bash
cargo run -p gdrive-optimize -- auth login
```

2. Run the command that actually needs broader access.

```bash
cargo run -p gdrive-optimize -- inspect exif <image-file-id>
cargo run -p gdrive-optimize -- unshare --shared-with anyone --apply --yes
cargo run -p gdrive-optimize -- trash --path '[orphan]/Coors/Model/*' --recursive --apply --yes
cargo run -p gdrive-optimize -- db remote push --yes
```

3. Confirm the broadened session only after the command succeeds.

```bash
cargo run -p gdrive-optimize -- auth status
```

## Recovery

- If you decline the prompt by mistake, simply rerun the command later.
- If you want to return to a narrower session, use `auth logout` and then `auth login` again.
