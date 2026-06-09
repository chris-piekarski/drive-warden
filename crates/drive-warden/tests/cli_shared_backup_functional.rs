mod support;

use support::*;

// drive_small contains one shared-with-me item (`inherited-file`), enough to
// exercise the backup -> declutter -> attention -> doctor flow end to end.

fn prepared(tmp: &tempfile::TempDir) {
    assert!(run_account_command(
        tmp,
        &["account", "add", "personal", "--empty", "--email", "mock@example.com"]
    )
    .status
    .success());
    assert!(run_account_in(tmp, "personal", &["auth", "login"]).status.success());
    assert!(run_account_in(tmp, "personal", &["sync"]).status.success());
}

#[test]
fn backup_declutter_attention_doctor_flow() {
    let tmp = tempfile::tempdir().expect("tempdir");
    prepared(&tmp);
    let backup_dir = tmp.path().join("bk");
    let backup = run_account_in(
        &tmp,
        "personal",
        &["backup", "shared-with-me", "--out", backup_dir.to_str().unwrap()],
    );
    assert!(backup.status.success(), "backup failed: {}", stderr(&backup));
    let manifest = backup_dir.join("manifest.jsonl");
    assert!(manifest.exists(), "manifest not written");

    // Declutter preview reads the backup manifest.
    let preview = run_account_in(
        &tmp,
        "personal",
        &["shared", "declutter", "--manifest", manifest.to_str().unwrap()],
    );
    assert!(preview.status.success(), "declutter preview failed: {}", stderr(&preview));

    // Attention report referencing the same manifest.
    let attention = run_account_in(
        &tmp,
        "personal",
        &["report", "attention", "--manifest", manifest.to_str().unwrap()],
    );
    assert!(attention.status.success(), "attention failed: {}", stderr(&attention));

    // Read-only health check.
    let doctor = run_account_in(&tmp, "personal", &["doctor"]);
    assert!(doctor.status.success(), "doctor failed: {}", stderr(&doctor));
}

#[test]
fn shared_declutter_apply_removes_backed_up_item() {
    let tmp = tempfile::tempdir().expect("tempdir");
    prepared(&tmp);
    let backup_dir = tmp.path().join("bk");
    assert!(run_account_in(
        &tmp,
        "personal",
        &["backup", "shared-with-me", "--out", backup_dir.to_str().unwrap()]
    )
    .status
    .success());
    let manifest = backup_dir.join("manifest.jsonl");

    // Apply (matching identity) runs the pre-mutation release + remove path.
    let apply = run_account_in(
        &tmp,
        "personal",
        &["shared", "declutter", "--manifest", manifest.to_str().unwrap(), "--apply", "--yes"],
    );
    assert!(apply.status.success(), "declutter apply failed: {}", stderr(&apply));
}

#[test]
fn backup_exports_shared_google_doc() {
    // A shared Google Doc exercises the export-attempt path in backup_item,
    // distinct from the binary-download path covered above.
    let fixture = "tests/fixtures/drive_shared_doc";
    let tmp = tempfile::tempdir().expect("tempdir");
    assert!(run_account_command_with_fixture(
        &tmp,
        fixture,
        &["account", "add", "personal", "--empty", "--email", "mock@example.com"]
    )
    .status
    .success());
    assert!(run_account_in_with_fixture(&tmp, fixture, "personal", &["auth", "login"])
        .status
        .success());
    assert!(run_account_in_with_fixture(&tmp, fixture, "personal", &["sync"]).status.success());

    let backup_dir = tmp.path().join("bk");
    let backup = run_account_in_with_fixture(
        &tmp,
        fixture,
        "personal",
        &["backup", "shared-with-me", "--out", backup_dir.to_str().unwrap()],
    );
    assert!(backup.status.success(), "backup failed: {}", stderr(&backup));
    let manifest = std::fs::read_to_string(backup_dir.join("manifest.jsonl")).expect("manifest");
    assert!(manifest.contains("shared-doc"), "manifest missing doc: {manifest}");
}
