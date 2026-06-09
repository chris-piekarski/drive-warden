# Multi-account Support Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Run multiple Google Drives (personal + work) as first-class, isolated accounts with a `gcloud`-style selector and a hard identity guard that refuses live mutations on account mismatch.

**Architecture:** Generalize today's single `./data/` directory into named `accounts/<name>/` dirs. `AppRuntime::from_cli` resolves an optional account context (flag → env → current pointer → legacy), synthesizing db/token/session/report paths from the account dir. A new `DriveGateway::get_account_profile()` primitive feeds an `ensure_account_identity` guard installed at the 5 low-level Drive-write functions (block) and read paths (warn). Adoption is an explicit `account add` step that moves an existing install into an account.

**Tech Stack:** Rust (clap, serde/toml, anyhow, tokio, rusqlite), workspace crates `gdrive-core` (trait + types), `gdrive-drive` (Google + mock gateways), `drive-warden` (CLI). Tests: cargo unit tests in-module + integration tests under `crates/drive-warden/tests/` using the `support` harness + mock fixtures.

**Reference spec:** `docs/superpowers/specs/2026-06-08-multi-account-design.md`

**Precedence correction (vs spec Section B):** the escape hatch is **`--db` only**. `--config` merely relocates the config file and does NOT disable account mode (so tests/users can set `[accounts] root` via config while staying in account mode).

---

## File Structure

- `crates/gdrive-core/src/lib.rs` — add `get_account_profile()` to `DriveGateway` (default impl `unimplemented`-style not allowed; provide real default via `get_account_about`? No — add as required method). Add `IdentityCheckMode` enum. (`AccountProfile` already exists at line 84.)
- `crates/gdrive-drive/src/lib.rs` — implement `get_account_profile()` for `GoogleDriveGateway` (reuse `account_from_about`, `ABOUT_FULL_FIELDS`) and `MockDriveGateway` (return active session account).
- `crates/drive-warden/src/account.rs` — **new module**: `AccountContext`, `AccountToml` (identity binding + overrides), name validation, load/save, `.current` pointer read/write, accounts-root resolution, adoption (file moves). Keeps `main.rs` from growing unwieldy.
- `crates/drive-warden/src/identity.rs` — **new module**: `ensure_account_identity()` guard + TOFU write-back + SECURITY ALERT formatting.
- `crates/drive-warden/src/main.rs` — `Cli` gains `--account`; `AppRuntime` gains `account: Option<AccountContext>`; `from_cli` resolution; `account` subcommand dispatch; guard calls at 5 write fns + read paths; Layer-2 surfacing in confirms/header; per-account backup/report defaults.
- `crates/drive-warden/tests/cli_account_functional.rs` — **new**: account add/list/use/current/show/remove + adoption.
- `crates/drive-warden/tests/cli_identity_guard_functional.rs` — **new**: SECURITY ALERT block + warn + success.
- `crates/drive-warden/tests/support/mod.rs` — add `run_account_command` helper (account mode, no `--db`).
- `Makefile`, `README.md`, `docs/operator/getting-started.md` — `--account` examples, `ACCOUNT` var.

---

## Phase 1 — Identity primitive (`get_account_profile`)

Smallest, self-contained, unblocks the guard.

### Task 1.1: Add `get_account_profile` to the gateway trait

**Files:**
- Modify: `crates/gdrive-core/src/lib.rs` (trait `DriveGateway`, ~line 1056)

- [ ] **Step 1 — Write failing test** (in `crates/gdrive-core/src/lib_tests.rs`, mock impl there already has a gateway): add a test asserting the trait object exposes `get_account_profile` returning the fixture account.

```rust
#[tokio::test]
async fn get_account_profile_returns_logged_in_account() {
    let gateway = test_gateway_logged_in(); // existing helper that logs the mock in
    let profile = gateway.get_account_profile().await.expect("profile");
    assert_eq!(profile.email, "operator@example.com");
    assert!(!profile.account_id.is_empty());
}
```

- [ ] **Step 2 — Run, verify fail**: `cargo test -p gdrive-core get_account_profile_returns_logged_in_account` → FAIL (method missing).

- [ ] **Step 3 — Add trait method** (required, no default):

```rust
// in pub trait DriveGateway, after login():
async fn get_account_profile(&self) -> CoreResult<AccountProfile>;
```

- [ ] **Step 4 — Implement for the in-module mock** (`lib_tests.rs` mock gateway): return its logged-in account profile.

- [ ] **Step 5 — Run, verify pass**; then `cargo build -p gdrive-core` (will fail until Task 1.2 implements the real gateways — that's expected; commit after 1.2).

### Task 1.2: Implement `get_account_profile` for Google + Mock gateways

**Files:**
- Modify: `crates/gdrive-drive/src/lib.rs` (Google impl near `get_account_about` ~817; Mock impl)

- [ ] **Step 1 — Google impl**: reuse the about call that already fetches `user(permissionId,emailAddress,displayName)`:

```rust
async fn get_account_profile(&self) -> CoreResult<AccountProfile> {
    self.ensure_scope(DriveScope::MetadataReadonly).await?;
    let about = self.about_get(ABOUT_FULL_FIELDS).await?; // existing about fetch path
    account_from_about(about) // existing fn at lib.rs:997
}
```
(Confirm the exact about-fetch helper name during impl; `account_from_about` already maps `About → AccountProfile`.)

- [ ] **Step 2 — Mock impl**: return the live session account:

```rust
async fn get_account_profile(&self) -> CoreResult<AccountProfile> {
    let session = self.require_active_session()?;
    self.validate_session(&self.fixture()?)?;
    Ok(session.account)
}
```

- [ ] **Step 3 — Build + run core tests**: `cargo test -p gdrive-core -p gdrive-drive` → PASS.

- [ ] **Step 4 — Commit**: `git add -A && git commit -m "feat(core): add DriveGateway::get_account_profile primitive\n\nRefs #1"`

---

## Phase 2 — Account model, config, path resolution

### Task 2.1: `account` module skeleton + `AccountToml` + name validation

**Files:**
- Create: `crates/drive-warden/src/account.rs`
- Modify: `crates/drive-warden/src/main.rs` (add `mod account;`)

- [ ] **Step 1 — Failing test** (in `account.rs` `#[cfg(test)]`):

```rust
#[test]
fn validates_account_names() {
    assert!(validate_account_name("work").is_ok());
    assert!(validate_account_name("personal-2").is_ok());
    assert!(validate_account_name("bad/name").is_err());
    assert!(validate_account_name("..").is_err());
    assert!(validate_account_name(".current").is_err());
    assert!(validate_account_name("").is_err());
}
```

- [ ] **Step 2 — Run, verify fail**: `cargo test -p drive-warden validates_account_names`.

- [ ] **Step 3 — Implement** `validate_account_name` (charset `[a-z0-9_-]+`, reject `.`/`..`/`/`/`.current`/empty), `AccountToml` (serde) with `schema_version`, `[identity]{state,email,account_id,display_name}`, `[remote]`, `[overrides]`, and `IdentityState` enum (`Unbound|Declared|Bound`). Add `load_account_toml(path)`, `save_account_toml(path, &AccountToml)` (atomic write via temp+rename).

- [ ] **Step 4 — Run, verify pass.**

- [ ] **Step 5 — Commit.**

### Task 2.2: Accounts-root resolution + `.current` pointer

**Files:** Modify `crates/drive-warden/src/account.rs`

- [ ] **Step 1 — Failing tests**: `accounts_root` resolves from (env `DRIVE_WARDEN_ACCOUNTS_ROOT` → config `[accounts] root` → default `data/accounts`); `read_current`/`write_current` round-trip via a `.current` file in a tempdir.

```rust
#[test]
fn current_pointer_roundtrips() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    assert_eq!(read_current(root).unwrap(), None);
    write_current(root, "work").unwrap();
    assert_eq!(read_current(root).unwrap().as_deref(), Some("work"));
}
```

- [ ] **Step 2 — Run fail; Step 3 — implement; Step 4 — pass; Step 5 — commit.**

### Task 2.3: `AccountContext` + selection precedence

**Files:** Modify `crates/drive-warden/src/account.rs`

- [ ] **Step 1 — Failing test** for precedence resolver `resolve_account_selection(flag, env, current)`:

```rust
#[test]
fn account_selection_precedence() {
    // flag wins over env and current
    assert_eq!(resolve_account_selection(Some("work"), Some("p"), Some("c")), Some("work".into()));
    // env beats current
    assert_eq!(resolve_account_selection(None, Some("env"), Some("c")), Some("env".into()));
    // current is the fallback
    assert_eq!(resolve_account_selection(None, None, Some("c")), Some("c".into()));
    // nothing selected
    assert_eq!(resolve_account_selection(None, None, None), None);
}
```

- [ ] **Steps 2-5**: fail → implement `resolve_account_selection` + `AccountContext { name, dir, toml, accounts_root }` with `load(accounts_root, name)` → pass → commit.

### Task 2.4: Wire account resolution into `AppRuntime::from_cli`

**Files:** Modify `crates/drive-warden/src/main.rs` (`Cli` ~602, `AppRuntime` ~1083, `from_cli` ~1144)

- [ ] **Step 1 — Failing test** (in main.rs `#[cfg(test)]` mod, ~3604): build a `Cli` with `--account work` + temp accounts root via config and assert `runtime.db_path` ends with `accounts/work/inventory.db` and `runtime.account` is `Some`. Also assert `--account` + `--db` returns an error.

- [ ] **Step 2 — Run fail.**

- [ ] **Step 3 — Implement:**
  - Add `#[arg(long, global = true)] account: Option<String>` to `Cli`.
  - In `from_cli`: error if `account.is_some() && db.is_some()`. Read env `DRIVE_WARDEN_ACCOUNT`. If `--db` present → legacy/escape-hatch path (today's behavior, `account = None`). Else resolve selection; if `Some(name)` → `AccountContext::load`, set `db_path = dir/inventory.db`, `runtime_dir = dir`, `reports_output_dir = dir/reports`, tokens/session under `dir`. If `None` and accounts root has accounts but no current → error "no current account; run `drive-warden account use <name>`". If `None` and no accounts root → legacy `data/`.
  - `credentials_path`: env → account `[overrides]` → shared `<accounts_root_parent>/credentials.json` (i.e. `data/credentials.json`).
  - Add `account: Option<AccountContext>` field to `AppRuntime`.

- [ ] **Step 4 — Run pass; Step 5 — `cargo build` + commit.**

---

## Phase 3 — Account subcommands + adoption

### Task 3.1: `account` subcommand enum + dispatch + `list`/`current`/`use`

**Files:** Modify `crates/drive-warden/src/main.rs` (Command enum, dispatch), `account.rs`

- [ ] **Step 1 — Failing functional test** `crates/drive-warden/tests/cli_account_functional.rs`:

```rust
mod support;
use support::*;

#[test]
fn account_add_empty_list_use_current() {
    let tmp = tempfile::tempdir().unwrap();
    // account add personal --empty ; account list shows it ; current is personal
    let out = run_account_command(&tmp, &["account", "add", "personal", "--empty", "--email", "me@x.com"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let list = run_account_command(&tmp, &["account", "list"]);
    assert!(stdout(&list).contains("personal"));
    let cur = run_account_command(&tmp, &["account", "current"]);
    assert!(stdout(&cur).contains("personal"));
}
```

- [ ] **Step 2 — Add `run_account_command` to support/mod.rs** (account mode: writes a temp mock config with `[accounts] root = <tmp>/accounts`, passes `--config` + `--backend mock` + `--account`-less for `account` subcommands; NO `--db`).

- [ ] **Step 3 — Run fail.**

- [ ] **Step 4 — Implement** `AccountCommand { Add, List, Use, Current, Show, Remove }` + `account_add(--empty|--email|--adopt*)`, `account_list` (reads each `account.toml` + db stat), `account_use` (validate exists, `write_current`), `account_current` (`read_current`). First account added → `write_current`.

- [ ] **Step 5 — Run pass; Step 6 — commit.**

### Task 3.2: `account show` + `account remove` (guarded)

- [ ] **Step 1 — Failing test**: `remove` refuses the current account; succeeds with `--yes` on a non-current account; is local-only.
- [ ] **Steps 2-5**: implement `account_show` (binding/paths/drift), `account_remove` (reject current, require `--yes`, delete dir, never touch remote) → pass → commit.

### Task 3.3: Adoption (move existing `data/` into an account)

**Files:** Modify `account.rs` (`adopt_into_account`), `main.rs` (`account add` adoption branch)

- [ ] **Step 1 — Failing test** (unit, tempdir): given a fake legacy layout (`data/inventory.db`, `google-tokens.json`, a `before-*` snapshot, `reports/x.md`), `adopt_into_account` moves them into `accounts/personal/` and leaves `credentials.json` at root; writes `account.toml` last; is idempotent/resumable (re-run repairs partial).

```rust
#[test]
fn adopts_legacy_layout() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    // seed legacy data/
    seed_legacy(root); // inventory.db, google-tokens.json, before-..., reports/a.md, credentials.json
    adopt_into_account(root, "personal", AdoptSource::LegacyData, Some("me@x.com")).unwrap();
    assert!(root.join("data/accounts/personal/inventory.db").exists());
    assert!(root.join("data/accounts/personal/google-tokens.json").exists());
    assert!(root.join("data/accounts/personal/reports/a.md").exists());
    assert!(root.join("data/credentials.json").exists()); // stays shared
    assert!(root.join("data/accounts/personal/account.toml").exists()); // sentinel last
}
```

- [ ] **Step 2 — Run fail.**

- [ ] **Step 3 — Implement** `adopt_into_account`: create target dir, move db + sidecars (`*.db-wal`,`*.db-shm`, `inventory.db.before-*`) + tokens + session + `reports/` tree; support `AdoptSource::{LegacyData, Explicit{db,tokens,session}}`; write `account.toml` as the final step; if target exists without valid `account.toml`, resume.

- [ ] **Step 4 — Wire into `account add`**: detect legacy `data/inventory.db` when not `--empty`; confirm interactively (show file list, `Proceed? [y/N]`) unless `--adopt`; in `--no-interactive` without `--adopt`/`--empty` and legacy present → error.

- [ ] **Step 5 — Functional test**: seed temp legacy, `account add personal --adopt --email me@x.com`, assert files moved + `account list` shows it. Run pass.

- [ ] **Step 6 — Adoption nudge**: `doctor` + `account list` warn when an un-adopted legacy `data/inventory.db` exists under an active accounts root. Test + commit.

---

## Phase 4 — Identity guard (two-layer)

### Task 4.1: `identity` module + `ensure_account_identity`

**Files:** Create `crates/drive-warden/src/identity.rs`; modify `main.rs` (`mod identity;`)

- [ ] **Step 1 — Unit tests** for the pure decision fn `evaluate_identity(state, bound_email, bound_id, live_email, live_id) -> IdentityOutcome`:

```rust
#[test]
fn identity_outcomes() {
    // bound: compare on account_id
    assert_eq!(evaluate_identity(Bound, "a@x", "ID1", "a@x", "ID1"), IdentityOutcome::Match);
    assert_eq!(evaluate_identity(Bound, "a@x", "ID1", "a@x", "ID2"), IdentityOutcome::Mismatch);
    // declared: compare on email; match -> BindNow(record id)
    assert_eq!(evaluate_identity(Declared, "a@x", "", "a@x", "ID1"), IdentityOutcome::BindNow);
    assert_eq!(evaluate_identity(Declared, "a@x", "", "b@y", "ID1"), IdentityOutcome::Mismatch);
    // unbound: TOFU -> BindNow
    assert_eq!(evaluate_identity(Unbound, "", "", "who@x", "ID9"), IdentityOutcome::BindNow);
}
```

- [ ] **Step 2 — Run fail; Step 3 — implement** `evaluate_identity` + `IdentityOutcome{Match,Mismatch,BindNow}`.

- [ ] **Step 4 — Implement `ensure_account_identity`** (async, takes `&dyn DriveGateway`, `&AppRuntime`, `IdentityCheckMode`):
  - If `runtime.account` is `None` → `Ok(())` (escape hatch unguarded).
  - Fetch `gateway.get_account_profile()`. On error: Block → return Err (fail closed); Warn → eprintln warning, `Ok(())`.
  - `evaluate_identity(...)`: `Match` → Ok. `BindNow` → `save_account_toml` with `state=Bound, account_id=live`, Ok. `Mismatch` → Block → `bail!("SECURITY ALERT: account '{name}' is bound to {bound} but the active Google session is {live}. Refusing.")`; Warn → eprintln + Ok.

- [ ] **Step 5 — Run pass; Step 6 — commit.**

### Task 4.2: Install guard at the 5 Drive-write boundaries + read paths

**Files:** Modify `crates/drive-warden/src/main.rs`

- [ ] **Step 1 — Functional test** `cli_identity_guard_functional.rs`:

```rust
mod support;
use support::*;

#[test]
fn declared_mismatch_blocks_trash_apply() {
    let tmp = tempfile::tempdir().unwrap();
    // fixture logs in as mock@example.com; bind a DIFFERENT declared email
    run_account_command(&tmp, &["account", "add", "personal", "--empty", "--email", "someone-else@example.com"]);
    run_account_in_personal(&tmp, &["auth", "login"]);
    run_account_in_personal(&tmp, &["sync"]);
    let out = run_account_in_personal(&tmp, &["trash", "--path", "*", "--recursive", "--apply", "--yes"]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("SECURITY ALERT"), "{}", stderr(&out));
}
```
(`run_account_in_personal` = account-mode invocation with `--account personal` against the temp root.)

- [ ] **Step 2 — Run fail** (mutation currently proceeds).

- [ ] **Step 3 — Insert guard calls (Block) at the 5 write fns:**
  - `create_remote_db_release` — first line (before `runtime.db_path.exists()` check at 2574).
  - `push_remote_db` (2464), `pull_remote_db` (2528), `rename_remote_db_folder` (2385), `apply_remote_db_release_prune` (2728) — first line.
  - Each: `ensure_account_identity(gateway, runtime, IdentityCheckMode::Block).await?;`

- [ ] **Step 4 — Insert guard calls (Warn)** at `sync`, `report`, and `build_remote_db_status` read entry.

- [ ] **Step 5 — Run pass.** Add success-path test: bind `--email mock@example.com` → trash apply proceeds (and account.toml becomes `Bound`). Add warn test: `sync` under mismatch warns but exits 0.

- [ ] **Step 6 — Commit.**

### Task 4.3: Layer-2 surfacing (header + confirm prompts)

**Files:** Modify `main.rs` (command header; `confirm_trash_apply`/`confirm_unshare_apply`/`confirm_move_apply`/`confirm_shared_declutter_apply` 3479-3551)

- [ ] **Step 1 — Test**: non-quiet, non-JSON run prints `account: personal (...)` header; trash confirm prompt names the account+email.
- [ ] **Steps 2-5**: thread `runtime.account` (name+bound email) into the header print (skip when `--quiet` or `--format json`; JSON envelope includes `account`) and into the four confirm fns. Pass → commit.

---

## Phase 5 — Glue, integration, docs

### Task 5.1: Per-account `backup` + `report` defaults

**Files:** `main.rs` (backup `--out` default 804; `resolve_report_dir` already uses `runtime.reports_output_dir`)

- [ ] **Step 1 — Test**: in account mode, `report all` (no `-o`) writes under `accounts/<name>/reports/`; `backup shared-with-me` default `--out` is under the account dir.
- [ ] **Step 2 — Implement**: make the backup `--out` default resolve from `runtime` (account dir) instead of the hardcoded `backups/shared-with-me`; confirm `reports_output_dir` is account-derived (done in 2.4). Pass → commit.

### Task 5.2: Two-account acceptance test

**Files:** Create `crates/drive-warden/tests/acceptance_multi_account.rs`; add a second fixture `tests/fixtures/drive_identity_b/api/mock-drive.json` (account `work@company.com`/`mock-work-1`).

- [ ] **Step 1 — Test**: adopt `personal` (fixture A) + add `work` (fixture B, `--email work@company.com`); sync+report each into isolated dirs; assert no cross-talk (personal db ≠ work db); attempt `work` op while `personal` session active → block.
- [ ] **Steps 2-4**: build fixture, implement test, run pass, commit.

### Task 5.3: Makefile + docs

**Files:** `Makefile` (`gdrive-sync`/`run`/`sync`/`report` accept `ACCOUNT ?=`), `README.md`, `docs/operator/getting-started.md`, `scripts/backup_shared_with_me.py` (defaults error helpfully).

- [ ] **Step 1**: Makefile targets pass `$(if $(ACCOUNT),--account $(ACCOUNT),)`.
- [ ] **Step 2**: README + getting-started gain an "Accounts" section with `account add/use/list` + `--account` examples and the SECURITY ALERT note.
- [ ] **Step 3**: update Python script arg defaults to fail with guidance instead of silently using `data/`.
- [ ] **Step 4 — Regenerate completions** if checked in; commit.

### Task 5.4: Full verification

- [ ] **Step 1**: `make fmt` (or `cargo fmt --all`).
- [ ] **Step 2**: `make lint` / `cargo clippy --all-targets --all-features -- -D warnings` → zero warnings.
- [ ] **Step 3**: `make test` (unit + functional + acceptance) → green.
- [ ] **Step 4**: `make build` → clean.
- [ ] **Step 5**: update PR #2 checklist; commit; push `feature/multi-account`.

---

## Self-Review

- **Spec coverage:** A (layout) → 2.4/3.x; B (precedence) → 2.3/2.4 (+ `--db`-only escape-hatch correction); C (adoption) → 3.3; D (guard, 5 boundaries, `get_account_profile`, TOFU, fail-closed, Layer 2) → Phase 1 + 4.1/4.2/4.3; E (account commands) → 3.1/3.2; F (glue/Makefile/scripts/docs) → 5.1/5.3; G (code changes) → all; H (testing) → 1.x, 2.x, 3.x, 4.2, 5.2; env var → 2.3/2.4; `--adopt`/`--empty` → 3.3.
- **Placeholders:** none — load-bearing tasks show real code; routine tasks show signatures + behavior. (During impl, confirm the Google `about_get` helper name reused by `get_account_profile`.)
- **Type consistency:** `IdentityState{Unbound,Declared,Bound}`, `IdentityOutcome{Match,Mismatch,BindNow}`, `IdentityCheckMode{Block,Warn}`, `AccountContext{name,dir,toml,accounts_root}`, `AccountToml` used consistently across Phases 2-4.
