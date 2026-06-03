mod support;

use gdrive_core::InventoryRepository;
use gdrive_db::SqliteInventoryRepository;

#[test]
fn trash_preview_and_apply_remove_file_from_snapshot() {
    let temp_dir = tempfile::TempDir::new().expect("tempdir");
    let login = support::run_mock_command(&temp_dir, &["auth", "login"]);
    assert!(login.status.success(), "stderr: {}", support::stderr(&login));
    let sync = support::run_mock_command(&temp_dir, &["sync"]);
    assert!(sync.status.success(), "stderr: {}", support::stderr(&sync));

    let preview =
        support::run_mock_command(&temp_dir, &["trash", "--path", "/Docs/PublicDeck.pdf"]);
    assert!(preview.status.success(), "stderr: {}", support::stderr(&preview));
    let stdout = support::stdout(&preview);
    assert!(stdout.contains("trash preview:"));
    assert!(stdout.contains("public-file"));
    assert!(stdout.contains("reason=actionable"));

    let apply = support::run_mock_command(
        &temp_dir,
        &["trash", "--path", "/Docs/PublicDeck.pdf", "--apply", "--yes"],
    );
    assert!(apply.status.success(), "stderr: {}", support::stderr(&apply));
    let stdout = support::stdout(&apply);
    assert!(stdout.contains("pre-mutation release: name=before-trash-"));
    assert!(stdout.contains("trash applied: planned=1 applied=1 skipped=0"));
    assert!(stdout.contains("post-apply sync:"));

    let repository =
        SqliteInventoryRepository::new(support::temp_db_path(&temp_dir)).expect("repo");
    let snapshot = repository.load_snapshot().expect("snapshot");
    assert!(!snapshot.files.iter().any(|file| file.id == "public-file"));
    let audit = repository.load_audit_log().expect("audit log");
    assert_eq!(audit.len(), 2);
    assert_eq!(audit[0].command, "trash");
    assert_eq!(audit[0].action, "trash_file_pending");
    assert_eq!(audit[0].file_id, "public-file");
    assert_eq!(audit[1].action, "trash_file");
    assert_eq!(audit[1].file_id, "public-file");

    let history = support::run_mock_command(&temp_dir, &["trash-history", "--limit", "5"]);
    assert!(history.status.success(), "stderr: {}", support::stderr(&history));
    let stdout = support::stdout(&history);
    assert!(stdout.contains("trash history: rows=1"));
    assert!(stdout.contains("/Docs/PublicDeck.pdf"));
    assert!(stdout.contains("recoverable_until="));

    let status = support::run_mock_command(&temp_dir, &["trash-status", "--within-days", "40"]);
    assert!(status.status.success(), "stderr: {}", support::stderr(&status));
    let stdout = support::stdout(&status);
    assert!(stdout.contains("trash status: total=1 pending=1"));
    assert!(stdout.contains("warnings=1"));

    let restore =
        support::run_mock_command(&temp_dir, &["trash-restore", "--file-id", "public-file"]);
    assert!(restore.status.success(), "stderr: {}", support::stderr(&restore));
    let stdout = support::stdout(&restore);
    assert!(stdout.contains("trash restore guidance: matches=1"));
    assert!(stdout.contains("CLI restore is not implemented"));
    assert!(stdout.contains("file_id=public-file"));

    let doctor = support::run_mock_command(&temp_dir, &["doctor", "--within-days", "40"]);
    assert!(doctor.status.success(), "stderr: {}", support::stderr(&doctor));
    let stdout = support::stdout(&doctor);
    assert!(stdout.contains("doctor: warnings="));
    assert!(stdout.contains("trash: total=1 pending=1"));
    assert!(stdout.contains("WARNING: 1 trash item(s) recoverability expires within 40 day(s)"));

    let releases = support::run_mock_command(&temp_dir, &["db", "remote", "release", "list"]);
    assert!(releases.status.success(), "stderr: {}", support::stderr(&releases));
    let stdout = support::stdout(&releases);
    assert!(stdout.contains("remote db releases: count=1"));
    assert!(stdout.contains("before-trash-"));
}

#[test]
fn trash_guards_non_interactive_and_conflicting_flags() {
    let temp_dir = tempfile::TempDir::new().expect("tempdir");
    let login = support::run_mock_command(&temp_dir, &["auth", "login"]);
    assert!(login.status.success(), "stderr: {}", support::stderr(&login));
    let sync = support::run_mock_command(&temp_dir, &["sync"]);
    assert!(sync.status.success(), "stderr: {}", support::stderr(&sync));

    let missing_yes = support::run_mock_command(
        &temp_dir,
        &["--no-interactive", "trash", "--path", "/Docs/PublicDeck.pdf", "--apply"],
    );
    assert!(!missing_yes.status.success());
    assert!(support::stderr(&missing_yes).contains("--yes"));

    let conflict = support::run_mock_command(
        &temp_dir,
        &["trash", "--path", "/Docs/PublicDeck.pdf", "--dry-run", "--apply", "--yes"],
    );
    assert!(!conflict.status.success());
    assert!(support::stderr(&conflict).contains("cannot be combined"));
}

#[test]
fn trash_folder_requires_recursive_flag() {
    let temp_dir = tempfile::TempDir::new().expect("tempdir");
    let login = support::run_mock_command(&temp_dir, &["auth", "login"]);
    assert!(login.status.success(), "stderr: {}", support::stderr(&login));
    let sync = support::run_mock_command(&temp_dir, &["sync"]);
    assert!(sync.status.success(), "stderr: {}", support::stderr(&sync));

    let preview = support::run_mock_command(&temp_dir, &["trash", "--path", "/Docs"]);
    assert!(preview.status.success(), "stderr: {}", support::stderr(&preview));
    let stdout = support::stdout(&preview);
    assert!(stdout.contains("reason=folder_without_recursive"));
    assert!(stdout.contains("actionable=no"));
}
