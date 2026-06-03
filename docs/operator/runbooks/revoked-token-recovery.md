# Revoked Token Recovery

Use this runbook when the live Google backend reports a revoked, expired, or otherwise invalid OAuth session.

## Symptoms

- `sync` fails with `revoked`, `expired`, or `invalid_grant`
- `auth status` no longer reflects a usable session
- a live command that previously worked now requests re-authentication guidance

## Recovery steps

1. Confirm you are using the intended live profile paths.

```bash
cargo run -p gdrive-optimize -- db stats
```

2. Log out to clear local session and token cache.

```bash
cargo run -p gdrive-optimize -- auth logout
```

3. Log in again.

```bash
cargo run -p gdrive-optimize -- auth login
```

4. Re-run a read-only validation command.

```bash
cargo run -p gdrive-optimize -- auth status
cargo run -p gdrive-optimize -- sync
```

## Notes

- `auth logout` deletes the local token cache and session metadata, then attempts best-effort token revocation.
- If you intentionally switched Google accounts, use a separate database/profile directory before logging in again.
- If the consent screen itself is failing, revisit [`../google-cloud-setup.md`](../google-cloud-setup.md).
