mod support;

use std::fs;
use std::path::{Path, PathBuf};

use support::*;

// The drive_small fixture authenticates as `mock@example.com` / `mock-account-1`.

fn personal_account_toml(tmp: &tempfile::TempDir) -> PathBuf {
    tmp.path().join("accounts/personal/account.toml")
}

/// Simulate a wrong binding (e.g. the account's tokens belong to a different
/// Google account than account.toml records).
fn write_bound_toml(path: &Path, email: &str, account_id: &str) {
    fs::write(
        path,
        format!(
            "schema_version = 1\n\n[identity]\nstate = \"bound\"\nemail = \"{email}\"\naccount_id = \"{account_id}\"\n"
        ),
    )
    .expect("write account.toml");
}

#[test]
fn declared_mismatch_rejected_at_login() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let add = run_account_command(
        &tmp,
        &["account", "add", "personal", "--empty", "--email", "someone-else@example.com"],
    );
    assert!(add.status.success(), "add failed: {}", stderr(&add));

    // Login authenticates as mock@example.com, which contradicts the declared email.
    let login = run_account_in(&tmp, "personal", &["auth", "login"]);
    assert!(!login.status.success(), "login should be rejected: {}", stdout(&login));
    assert!(stderr(&login).contains("SECURITY ALERT"), "stderr was: {}", stderr(&login));
}

#[test]
fn matching_identity_logs_in_and_binds() {
    let tmp = tempfile::tempdir().expect("tempdir");
    assert!(run_account_command(
        &tmp,
        &["account", "add", "personal", "--empty", "--email", "mock@example.com"]
    )
    .status
    .success());

    let login = run_account_in(&tmp, "personal", &["auth", "login"]);
    assert!(login.status.success(), "login failed: {}", stderr(&login));

    let show = run_account_command(&tmp, &["account", "show", "personal"]);
    assert!(stdout(&show).contains("bound"), "show: {}", stdout(&show));
    assert!(stdout(&show).contains("mock@example.com"), "show: {}", stdout(&show));
}

#[test]
fn bound_mismatch_blocks_trash_apply() {
    let tmp = tempfile::tempdir().expect("tempdir");
    assert!(run_account_command(&tmp, &["account", "add", "personal", "--empty"]).status.success());
    assert!(run_account_in(&tmp, "personal", &["auth", "login"]).status.success());
    assert!(run_account_in(&tmp, "personal", &["sync"]).status.success());

    // Re-point the binding to a different identity than the live session.
    write_bound_toml(&personal_account_toml(&tmp), "wrong@example.com", "wrong-account-id");

    // The destructive op this feature exists to guard must be refused.
    let trash = run_account_in(
        &tmp,
        "personal",
        &["trash", "--path", "/Docs/PublicDeck.pdf", "--apply", "--yes"],
    );
    assert!(!trash.status.success(), "trash should be blocked: {}", stdout(&trash));
    assert!(stderr(&trash).contains("SECURITY ALERT"), "stderr was: {}", stderr(&trash));
}

#[test]
fn matching_identity_allows_trash_apply() {
    let tmp = tempfile::tempdir().expect("tempdir");
    assert!(run_account_command(
        &tmp,
        &["account", "add", "personal", "--empty", "--email", "mock@example.com"]
    )
    .status
    .success());
    assert!(run_account_in(&tmp, "personal", &["auth", "login"]).status.success());
    assert!(run_account_in(&tmp, "personal", &["sync"]).status.success());

    // A correctly-bound account proceeds through the write-boundary guard.
    let trash = run_account_in(
        &tmp,
        "personal",
        &["trash", "--path", "/Docs/PublicDeck.pdf", "--apply", "--yes"],
    );
    assert!(trash.status.success(), "trash should proceed: {}", stderr(&trash));
    assert!(stdout(&trash).contains("before-trash-"), "stdout: {}", stdout(&trash));
}

#[test]
fn bound_mismatch_warns_on_sync_but_proceeds() {
    let tmp = tempfile::tempdir().expect("tempdir");
    assert!(run_account_command(&tmp, &["account", "add", "personal", "--empty"]).status.success());
    assert!(run_account_in(&tmp, "personal", &["auth", "login"]).status.success());
    assert!(run_account_in(&tmp, "personal", &["sync"]).status.success());
    write_bound_toml(&personal_account_toml(&tmp), "wrong@example.com", "wrong-account-id");

    // sync is read-class: it warns about the mismatch but is not blocked.
    let sync = run_account_in(&tmp, "personal", &["sync"]);
    assert!(sync.status.success(), "sync should proceed (warn-only): {}", stderr(&sync));
    assert!(stderr(&sync).contains("SECURITY ALERT"), "stderr was: {}", stderr(&sync));
}

#[test]
fn account_header_is_surfaced() {
    let tmp = tempfile::tempdir().expect("tempdir");
    assert!(run_account_command(
        &tmp,
        &["account", "add", "personal", "--empty", "--email", "mock@example.com"]
    )
    .status
    .success());
    assert!(run_account_in(&tmp, "personal", &["auth", "login"]).status.success());
    assert!(run_account_in(&tmp, "personal", &["sync"]).status.success());

    // Layer 2: an account-mode command surfaces the active account on stderr.
    let preview = run_account_in(&tmp, "personal", &["trash", "--path", "/Docs/PublicDeck.pdf"]);
    assert!(preview.status.success(), "preview failed: {}", stderr(&preview));
    assert!(stderr(&preview).contains("account: personal"), "stderr was: {}", stderr(&preview));
}
