# Invalid Page Token Recovery

Use this runbook when delta sync fails with `invalid page token` or `410 Gone`.

## Why this happens

Google Drive change tokens can become unusable after enough time or state churn. When that happens, the local committed snapshot is still safe, but the next delta cannot continue from the old token.

## Recovery steps

1. Do not delete the database manually.
2. Rebuild the snapshot from a full sync.

```bash
cargo run -p drive-warden -- sync --full
```

3. Verify the new committed token and snapshot health.

```bash
cargo run -p drive-warden -- db stats
```

4. Re-run the discovery or reporting command you were working on.

```bash
cargo run -p drive-warden -- report summary -o reports/recovery
```

## Notes

- The repository keeps the previously committed snapshot until the replacement sync commits successfully.
- A failed delta should not leave the local cache half-applied.
- If repeated `sync --full` attempts fail, verify the OAuth session first with [`revoked-token-recovery.md`](./revoked-token-recovery.md).
