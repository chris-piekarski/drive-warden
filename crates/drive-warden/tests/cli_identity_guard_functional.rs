mod support;

use support::*;

// The drive_small fixture authenticates as `mock@example.com` / `mock-account-1`.

#[test]
fn declared_mismatch_blocks_remote_push() {
    let tmp = tempfile::tempdir().expect("tempdir");
    // Declare an email that differs from the fixture's login identity.
    let add = run_account_command(
        &tmp,
        &["account", "add", "personal", "--empty", "--email", "someone-else@example.com"],
    );
    assert!(add.status.success(), "add failed: {}", stderr(&add));

    let login = run_account_in(&tmp, "personal", &["auth", "login"]);
    assert!(login.status.success(), "login failed: {}", stderr(&login));

    // A remote-DB write must be refused with a SECURITY ALERT.
    let push = run_account_in(&tmp, "personal", &["db", "remote", "push", "--yes"]);
    assert!(!push.status.success(), "push unexpectedly succeeded: {}", stdout(&push));
    assert!(stderr(&push).contains("SECURITY ALERT"), "stderr was: {}", stderr(&push));
}

#[test]
fn matching_identity_binds_account() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let add = run_account_command(
        &tmp,
        &["account", "add", "personal", "--empty", "--email", "mock@example.com"],
    );
    assert!(add.status.success(), "add failed: {}", stderr(&add));
    assert!(run_account_in(&tmp, "personal", &["auth", "login"]).status.success());

    let sync = run_account_in(&tmp, "personal", &["sync"]);
    assert!(sync.status.success(), "sync failed: {}", stderr(&sync));

    // A matching identity binds the account (declared -> bound, permissionId recorded).
    let show = run_account_command(&tmp, &["account", "show", "personal"]);
    assert!(stdout(&show).contains("bound"), "show: {}", stdout(&show));
    assert!(stdout(&show).contains("mock@example.com"), "show: {}", stdout(&show));
}

#[test]
fn mismatch_warns_on_sync_but_proceeds() {
    let tmp = tempfile::tempdir().expect("tempdir");
    run_account_command(
        &tmp,
        &["account", "add", "personal", "--empty", "--email", "someone-else@example.com"],
    );
    assert!(run_account_in(&tmp, "personal", &["auth", "login"]).status.success());

    // sync is read-class: it warns about the mismatch but is not blocked.
    let sync = run_account_in(&tmp, "personal", &["sync"]);
    assert!(sync.status.success(), "sync should proceed (warn-only): {}", stderr(&sync));
    assert!(stderr(&sync).contains("SECURITY ALERT"), "stderr was: {}", stderr(&sync));
}
