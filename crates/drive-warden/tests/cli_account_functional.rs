mod support;

use support::*;

#[test]
fn account_add_empty_list_use_current() {
    let tmp = tempfile::tempdir().expect("tempdir");

    let add = run_account_command(
        &tmp,
        &["account", "add", "personal", "--empty", "--email", "me@x.com"],
    );
    assert!(add.status.success(), "add failed: {}", stderr(&add));
    assert!(stdout(&add).contains("Created account"), "{}", stdout(&add));

    let list = run_account_command(&tmp, &["account", "list"]);
    assert!(list.status.success(), "list failed: {}", stderr(&list));
    assert!(stdout(&list).contains("personal"), "list output: {}", stdout(&list));

    let current = run_account_command(&tmp, &["account", "current"]);
    assert!(stdout(&current).contains("personal"), "current: {}", stdout(&current));

    let add_work = run_account_command(&tmp, &["account", "add", "work", "--empty"]);
    assert!(add_work.status.success(), "{}", stderr(&add_work));
    let use_work = run_account_command(&tmp, &["account", "use", "work"]);
    assert!(use_work.status.success(), "{}", stderr(&use_work));

    let current2 = run_account_command(&tmp, &["account", "current"]);
    assert!(stdout(&current2).contains("work"), "current2: {}", stdout(&current2));
}

#[test]
fn account_remove_refuses_current_and_requires_yes() {
    let tmp = tempfile::tempdir().expect("tempdir");
    run_account_command(&tmp, &["account", "add", "personal", "--empty"]);

    // personal is the current (first) account; removal must be refused.
    let removed = run_account_command(&tmp, &["account", "remove", "personal", "--yes"]);
    assert!(!removed.status.success());
    assert!(stderr(&removed).contains("current account"), "{}", stderr(&removed));

    // A non-current account still requires --yes.
    run_account_command(&tmp, &["account", "add", "work", "--empty"]);
    let no_yes = run_account_command(&tmp, &["account", "remove", "work"]);
    assert!(!no_yes.status.success());
    assert!(stderr(&no_yes).contains("--yes"), "{}", stderr(&no_yes));

    let yes = run_account_command(&tmp, &["account", "remove", "work", "--yes"]);
    assert!(yes.status.success(), "{}", stderr(&yes));
}

#[test]
fn account_add_adopts_legacy_database() {
    let tmp = tempfile::tempdir().expect("tempdir");
    // Seed a legacy data/ db under the temp accounts-root parent.
    let data_dir = tmp.path().join("accounts").parent().unwrap().to_path_buf();
    // accounts root is <tmp>/accounts, so its parent is <tmp>; legacy db lives at <tmp>/inventory.db
    std::fs::write(data_dir.join("inventory.db"), b"legacy-db").expect("seed db");

    let add = run_account_command(
        &tmp,
        &["account", "add", "personal", "--adopt", "--email", "me@x.com"],
    );
    assert!(add.status.success(), "adopt failed: {}", stderr(&add));
    assert!(stdout(&add).contains("Adopted"), "{}", stdout(&add));

    // The db moved into the account dir and out of the legacy location.
    assert!(tmp.path().join("accounts/personal/inventory.db").exists());
    assert!(!tmp.path().join("inventory.db").exists());
}
