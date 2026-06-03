# Google Cloud Setup

Use this guide to prepare a Google Cloud project for the supported live `My Drive` workflow.

## Support level

- supported: live Google backend for `My Drive`
- supported: installed-app OAuth with incremental scope upgrades
- deferred: Shared Drives

The mock backend is still the offline release gate and acceptance surface. Use the live backend only for operator-controlled smoke validation and real-account work.

## Required Google Cloud setup

1. Create or select a Google Cloud project.
2. Enable the Google Drive API for that project.
3. Configure an OAuth consent screen appropriate for a Desktop application.
4. Create an OAuth 2.0 Client ID of type `Desktop app`.
5. Download the resulting credential JSON file.

## Recommended local layout

```text
data/
├── credentials.json
├── google-session.json
├── google-tokens.json
└── inventory.db
```

The defaults assume `credentials.json` lives beside the selected database path. You can override credentials, token, and session paths in config or via environment variables.

## Config example

```toml
[backend]
kind = "google"

[database]
path = "data/inventory.db"
remote_folder_name = "gdrive-optimize-db"

[google]
credentials_path = "data/credentials.json"
token_path = "data/google-tokens.json"
session_path = "data/google-session.json"
```

Environment overrides:

- `GDRIVE_OPTIMIZE_CREDENTIALS`
- `GDRIVE_OPTIMIZE_TOKENS`
- `GDRIVE_OPTIMIZE_GOOGLE_SESSION`

## Scope model

The live backend uses least-privilege progression:

- `https://www.googleapis.com/auth/drive.metadata.readonly`
  Requested by `auth login`, and used by `sync`, `find`, and `report`.
- `https://www.googleapis.com/auth/drive.readonly`
  Requested when `inspect exif` or `db remote pull` needs broader read access than the current session has.
- `https://www.googleapis.com/auth/drive`
  Requested when `unshare --apply`, `trash --apply`, or `db remote push` needs mutation access.

The broader scope flow does not replace the stored session unless consent completes successfully.

## What to expect during login

- the CLI opens or presents a browser-based Google consent flow
- Google redirects back to the installed-app local callback flow handled by `yup-oauth2`
- refreshable tokens are cached in the configured token file
- session metadata is cached separately so the CLI can report active scopes and account identity
- `auth logout` deletes local session/token state and attempts best-effort token revocation

## Operational guidance

- Keep `credentials.json` out of version control.
- Use one database path per environment/profile so the inventory snapshot, token cache, and session metadata stay aligned.
- Keep the configured remote DB folder private. `db remote` commands abort with `SECURITY ALERT` if the folder, database file, or manifest file is shared.
- Prefer `auth logout` before switching accounts in the same profile directory.
- Use the mock backend for CI, demos, and coverage gates.
- Keep real-account smoke checks small and controlled. See [`../testing/live-smoke.md`](../testing/live-smoke.md).

## Troubleshooting checklist

- Missing `credentials.json`
  Verify the configured `credentials_path` exists and contains a Desktop OAuth client.
- Consent screen rejects the app
  Confirm the Drive API is enabled and the OAuth consent screen is configured for the intended user population.
- `revoked or expired`
  Re-run `auth login`. See [`runbooks/revoked-token-recovery.md`](runbooks/revoked-token-recovery.md).
- `invalid page token` or `410 Gone`
  Run `sync --full`. See [`runbooks/invalid-page-token-recovery.md`](runbooks/invalid-page-token-recovery.md).
- Scope upgrade fails
  Retry the command that needs the broader scope. If consent is declined, the previous narrower session is left in place.
