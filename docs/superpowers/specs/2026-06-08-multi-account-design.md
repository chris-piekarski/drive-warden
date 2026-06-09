# Multi-account support (personal + work) — Design

**Status:** Draft for review
**Date:** 2026-06-08
**Author:** Chris Piekarski (with Claude)

## Goal

Let one operator run drive-warden against multiple Google Drives (e.g. `personal`
and `work`) as a first-class feature, with isolated databases, tokens, reports,
and remote-DB sync per account — and a hard safety guard that refuses destructive
operations when the live Google session does not match the account the operator
selected. Selecting and switching accounts should feel like `gcloud`/`kubectl`.

## Background

Today the whole of a single account lives in `./data/` (gitignored):
`inventory.db` (+ `.db-wal`/`.db-shm` + `before-*` snapshots), `google-tokens.json`,
`google-session.json`, and a shared `credentials.json`. Reports default to a
top-level `./reports/`. Path resolution in `AppRuntime::from_cli`
(`crates/drive-warden/src/main.rs:1143-1212`) derives `runtime_dir` from the
parent of the db path, and tokens/session/mock-state default under it. So the
existing model is already "an account = a directory" — `data/` is just that
directory. This feature generalizes that one fixed folder into named
`accounts/<name>/` directories.

## Decisions (locked during brainstorming)

1. **Selection:** named `--account <name>` global flag plus a persisted "current"
   pointer (`gcloud`/`kubectl` model). Day-to-day you run bare commands against
   the current account; `--account` overrides for one invocation.
2. **Layout:** directory per account.
3. **Root:** configurable, default `./data/accounts/<name>/` (stays under today's
   `data/`).
4. **Identity guard:** each account binds to an expected Google identity;
   live mutations **hard-block** on mismatch. Reads **warn**.
5. **Adoption (supersedes "auto-migrate"):** existing databases are brought into
   the account model by an **explicit, named** step (`account add <name>`),
   never automatically.
6. **Scope:** one account per command for v1. No `--all-accounts`.

## Section A — Account model & disk layout

```
data/
├── config.toml                     # GLOBAL config (accounts root, backend, shared creds, defaults)
├── credentials.json                # SHARED OAuth client (one app, all accounts)
└── accounts/
    ├── .current                    # one line: the current account name
    ├── personal/
    │   ├── account.toml            # identity binding + per-account overrides
    │   ├── inventory.db            # (+ .db-wal / .db-shm / before-* snapshots)
    │   ├── google-tokens.json      (chmod 600)
    │   ├── google-session.json     (chmod 600)
    │   └── reports/                # per-account report output
    └── work/
        └── … same shape …
```

- `runtime_dir` logic is unchanged — it still derives db/tokens/session from one
  directory. That directory is now `accounts/<name>/` instead of `data/`.
- `credentials.json` stays **shared at the root** (it is app infrastructure, not
  per-user). Resolution: `DRIVE_WARDEN_CREDENTIALS` env → per-account override in
  `account.toml` → shared `data/credentials.json`.

### `account.toml` schema

```toml
schema_version = 1

[identity]
state = "bound"            # "unbound" | "declared" | "bound"
email = "me@gmail.com"     # declared and/or observed email
account_id = "01234567"    # Google permissionId — the durable identity (set on first bind)
display_name = "Chris P."  # optional, for display

[remote]                   # all optional; default to today's behavior
folder_name = "drive-warden-db"
db_name = "inventory.db"

[overrides]                # all optional
credentials_path = "…"     # rarely used; usually share the root one
reports_dir = "…"          # defaults to <accountdir>/reports
```

### Global `data/config.toml` additions

```toml
[accounts]
root = "data/accounts"     # configurable account root

# Existing sections remain valid but are LEGACY-MODE ONLY (see Section B):
# [backend], [google] credentials_path, [reports] stale_threshold_days are global.
# [database] path, [google] token_path/session_path are ignored in account mode.
```

The current-account pointer lives in a dedicated `data/accounts/.current` file
(not in `config.toml`) so `account use` never rewrites/clobbers the user's config
comments.

## Section B — Selection & precedence

New global flag `--account <name>` alongside existing `--config`/`--db`.

**Resolution order (most explicit wins):**

1. **Explicit `--db` / `--config`** — full escape hatch. Preserves today's behavior
   verbatim so the mock test suite (`--config tests/config/mock.toml`) and any
   scripted path keep working untouched. No account, no guard (opt-out).
2. **`--account <name>` flag** — synthesizes all paths from
   `<root>/accounts/<name>/`, loads that account's `account.toml`.
3. **`DRIVE_WARDEN_ACCOUNT` env var** — same effect as `--account`, for
   per-terminal separation (`export DRIVE_WARDEN_ACCOUNT=work`). The `--account`
   flag overrides it; it does not change the saved `current` pointer.
4. **`current` pointer** (`data/accounts/.current`) — when no flag or env var is set.
5. **Legacy `./data/`** — only when no accounts root exists yet and no account is
   selected. A brand-new or never-adopted install keeps working exactly like today.

`--account` and explicit `--db` are **mutually exclusive** — passing both is a hard
error, never a silent pick. Explicit `--db`/`--config` (step 1) also overrides the
env var.

### Config layering: global vs per-account

- **Global (top-level `config.toml`):** `[accounts] root`, `[backend] kind`,
  `[google] credentials_path` (shared OAuth client), `[reports] stale_threshold_days`.
- **Per-account (`account.toml`):** identity binding, `[remote]` overrides,
  `[overrides]`.
- **Derived (never from config in account mode):** `db_path =
  <accountdir>/inventory.db`; tokens/session under `<accountdir>/`; reports default
  to `<accountdir>/reports`.
- **Stale top-level `[database] path` / `[google] token_path` / `session_path`:**
  **ignored in account mode.** They are consulted only in legacy mode (resolution
  step 4). This prevents a leftover `path = "data/inventory.db"` from hijacking an
  account. `[google] credentials_path` is the one exception — it is the shared
  client and is consulted in both modes.

## Section C — Adoption (explicit, named)

Adoption brings an existing database into the account model. Nothing moves until
the operator asks. Driven by `account add`:

- **Default `data/` install:** `drive-warden account add personal` detects legacy
  `data/inventory.db` and adopts it — **moves** the db (+ `.db-wal`/`.db-shm`,
  `before-*` snapshots), `google-tokens.json`, `google-session.json`, and the
  top-level `reports/` tree into `accounts/personal/`. `credentials.json` stays
  shared at the root. Prints exactly what moved; confirms first (it is a file move).

**Confirmation:** adoption shows the exact file list and asks `Proceed? [y/N]` by
default. `--adopt` pre-approves for non-interactive/scripted runs; `--empty`
creates a fresh account instead of adopting. In non-interactive mode with a legacy
`data/` present and neither flag given, it errors and asks for an explicit choice.
- **An existing db elsewhere** (e.g. a second drive stood up via the manual
  two-config approach): `account add work --adopt-db <path> --adopt-tokens <path>
  [--adopt-session <path>]` adopts that specific set.
- **Brand-new account, nothing to adopt:** `account add work --email work@co.com
  --empty` creates the dir only; `auth login` populates it.

**Atomicity & resume:** adoption moves files into the target account dir, then
writes `account.toml` **last** as a completion sentinel. If `accounts/<name>/`
exists without a valid `account.toml`, the account is treated as
incompletely-adopted and the next `account add <name>` resumes/repairs rather than
double-moving.

**Safety net:** `doctor` and `account list` detect an un-adopted legacy
`data/inventory.db` still present and nudge the operator to adopt it, so it never
silently rots once an accounts root exists.

The first account added becomes `current` automatically.

## Section D — Identity & the two-layer safety guard

The marquee feature. Two layers because they catch different failures.

### Layer 1 — Hard-block at the Drive-write boundaries (catches misconfiguration)

There is **no single chokepoint.** Two families of operations write to a selected
account's Drive, and they do **not** share a call path:

- **File mutations** (`trash`/`unshare`/`move`/`shared declutter` `--apply`) funnel
  through `create_pre_mutation_release` → `create_remote_db_release`, which itself
  **uploads a DB snapshot to the account's Drive** (`main.rs:2615`) *before* the
  file mutation runs.
- **Remote-DB custody** (`db remote push`/`sync`/`pull`/`rename-folder`/`release`/
  `prune`) writes via `push_remote_db`, `pull_remote_db`, `rename_remote_db_folder`,
  `apply_remote_db_release_prune` — independent of the pre-mutation path.

The guard is therefore installed at the **5 low-level Drive-write functions**, not
their callers (so no command can bypass it), and **before** any
`load_remote_db_endpoint`/upload:

1. `create_remote_db_release` (covers all 4 file-mutation applies + `db remote
   release` create)
2. `push_remote_db`
3. `pull_remote_db`
4. `rename_remote_db_folder`
5. `apply_remote_db_release_prune`

A new helper:

```rust
async fn ensure_account_identity(
    gateway: &dyn DriveGateway,
    runtime: &AppRuntime,
    mode: IdentityCheckMode, // Block (writes) | Warn (reads)
) -> Result<()>
```

- **No account context** (escape-hatch/legacy): returns Ok — unguarded, as today.
- **Block mode, profile fetch fails:** **fail closed** — return Err, refuse the
  mutation. (Do not reuse the existing best-effort `fetch_account_about_best_effort`
  semantics, which return `None` on error.)
- Compares against `account.toml` per identity state (below).

### Required core primitive (gap found in review)

`AccountAbout` is **quota-only** (`crates/gdrive-core/src/lib.rs:582-589`) — it does
not carry the user's email/permissionId. The live profile only comes out of
`login()` today. So add a dedicated accessor to the gateway trait:

```rust
async fn get_account_profile(&self) -> CoreResult<AccountProfile>; // {account_id, email, display_name}
```

- **Google impl:** `ensure_scope(MetadataReadonly)` (already granted at login, no
  new consent) → `about.get` with the `user(permissionId,emailAddress,displayName)`
  fields already present in `ABOUT_FIELDS` (`gdrive-drive/src/lib.rs:21`) → reuse
  `account_from_about` (`lib.rs:997-1007`).
- **Mock impl:** derive from the fixture's session/about so tests can present a
  mismatched identity.

The durable identity is the **permissionId** (`account_id`), stable across email
changes. Email is for display and for the pre-login declared check.

### Identity state machine & comparison keys

| `account.toml` state | How reached | Block-mode behavior |
|---|---|---|
| **declared** | `account add --email <e>` (no login yet) | Live `email` must equal declared `email`; on first successful match, record `account_id` and transition to **bound**. Mismatch → SECURITY ALERT. |
| **bound** | after first successful identity observation | Live `account_id` must equal stored `account_id` (permissionId). Mismatch → SECURITY ALERT. |
| **unbound** | bare adoption (`account add` without `--email`) | **TOFU:** record live `{email, account_id}`, transition to **bound**, proceed. Cannot catch a wrong-account *first* mutation — see note. |

**Blessed path:** `account add <name> --email <e>` has no unbound window and is the
recommended way to add an account. Bare adoption is a convenience whose *first* use
is protected only by Layer 2.

### Layer 2 — Surface active identity everywhere (catches wrong-account-selected)

When the binding matches, Layer 1 is silent — so simply picking the wrong account
sails through. Defense is visibility:

- **Command header** (non-quiet, non-JSON): `▶ account: work (work@company.com)`.
  In `--format json`, include the account in the JSON envelope instead.
- **Apply confirmations** (`confirm_trash_apply` / `confirm_unshare_apply` /
  `confirm_move_apply` / `confirm_shared_declutter_apply`, `main.rs:3479-3551`) name
  the account + email: *"About to trash 12 files on **work** (work@company.com).
  Continue? [y/N]"*.
- **`doctor`** reports active account, bound vs. live email, token presence,
  credentials resolution, and flags drift.

### Scope escalation note

`login` grants `MetadataReadonly`; mutations call `ensure_scope(DriveScope::Drive)`
on demand (`gdrive-drive/src/lib.rs:472+`). A freshly-added account's **first**
write triggers an interactive `Drive`-scope consent that writes back to that
account's token file. Expected, documented behavior. The guard's `get_account_profile`
needs only metadata scope, so it never triggers extra consent.

## Section E — Account management commands

```
drive-warden account add <name> [--email <e>] [--empty] [--adopt]
                                [--adopt-db <p>] [--adopt-tokens <p>] [--adopt-session <p>]
drive-warden account list           # names, emails, current marker, db size, last-sync (mtime); local-only
drive-warden account use <name>     # set current pointer
drive-warden account current        # print active account
drive-warden account show [<name>]  # binding + paths + drift
drive-warden account remove <name>  # local-only delete; guarded
```

- **`account list`/`show`** read `account.toml` + local db stats only — no network.
- **`account remove`** is **local-only** (never deletes the remote `drive-warden-db`
  backup in that Drive), refuses to remove the **current** account (switch first),
  requires `--yes`, and prints what will be deleted.
- **Name validation:** `[a-z0-9_-]+`, no `.`/`..`/slashes, reserved names rejected
  (e.g. `.current`).

## Section F — Commands / Makefile / scripts / docs updates

- **`report` default dir:** `resolve_report_dir` (`main.rs:1328`) already uses
  `runtime.reports_output_dir`; in account mode that field defaults to
  `<accountdir>/reports`. Single point — no resolver change.
- **`backup shared-with-me --out` default collision:** the hardcoded default
  `backups/shared-with-me` (`main.rs:804`) would have two accounts writing the same
  path. Change the default to derive from the account dir
  (`<accountdir>/backups/shared-with-me`).
- **Makefile:** `gdrive-sync`, `run`, `sync`, `report` targets gain an optional
  `ACCOUNT` var passed through as `--account` (e.g. `make gdrive-sync ACCOUNT=work`).
- **`scripts/backup_shared_with_me.py`:** standalone, not referenced anywhere
  (Makefile/docs/CI). Low priority. Update its `--db/--credentials/--tokens` defaults
  to error helpfully (point at the accounts model) rather than silently using
  `data/…`.
- **Docs:** README quick start + getting-started gain an "accounts" section;
  `--account` shown in examples. Regenerate shell completions for the new flag and
  `account` subcommands (static; dynamic name completion is out of scope).

## Section G — Concrete code changes

- `gdrive-core`: add `DriveGateway::get_account_profile()`; `IdentityCheckMode`.
- `gdrive-drive`: Google + mock impls of `get_account_profile`.
- `drive-warden/main.rs`:
  - `Cli`: add `--account` global; read `DRIVE_WARDEN_ACCOUNT` env as a fallback
    (flag wins); mutual-exclusion of `--account`/env with `--db`.
  - `AppRuntime`: add `account: Option<AccountContext>` carrying name, dir, and
    loaded `account.toml` binding; `from_cli` resolves per Section B and loads
    `account.toml`.
  - `ensure_account_identity(...)` helper + TOFU write-back of `account.toml`.
  - Call the guard at the 5 write functions (Block) and in `sync`/`report`/`db
    remote status` (Warn).
  - `account` subcommand module (add/list/use/current/show/remove) + adoption.
  - Layer-2 surfacing in headers and the 4 confirm fns.
  - Per-account `backup` default; reports default via `reports_output_dir`.

## Section H — Testing strategy

- **Unit:** path/precedence resolution (flag vs current vs legacy vs escape-hatch);
  `--account`+`--db` mutual-exclusion error; `account.toml` parse + state machine;
  name validation; adoption move + resume-on-partial (temp dirs); config layering
  (stale `[database] path` ignored in account mode).
- **Guard (mock):** add a second fixture/profile with a distinct identity. Assert:
  bound mismatch → SECURITY ALERT bail at each of the 5 write boundaries; declared
  mismatch pre-login → bail; unbound TOFU records + proceeds; Block-mode
  profile-fetch failure → fail closed; Warn paths proceed with warning.
- **Functional/integration (mock):** `account add/list/use/remove`; `--account`
  routing isolates db/tokens/reports; two accounts coexist without cross-talk;
  `db remote push` on wrong identity refuses.
- **Acceptance:** two-account end-to-end on mock fixtures — adopt `personal`, add
  `work`, sync + report each, attempt a cross-account mutation and confirm the
  block.

## Out of scope / YAGNI (v1)

- `--all-accounts` fan-out (any command). Loop in the shell instead.
- Shared Drives (already deferred project-wide).
- Dynamic shell completion of account *names*.
- Auto-deriving `backup --out` per *named backup* beyond the per-account default.

## Resolved decisions (formerly open)

1. **`DRIVE_WARDEN_ACCOUNT` env var:** **included** as a low-precedence override
   (below the `--account` flag, above the saved `current` pointer) for per-terminal
   separation. Does not change the saved default.
2. **Adoption confirmation:** **confirm-first by default** — show the file list and
   ask `Proceed? [y/N]`; `--adopt` pre-approves for non-interactive runs; `--empty`
   creates a fresh account instead of adopting.
