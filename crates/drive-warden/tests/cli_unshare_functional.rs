mod support;

use std::fs;

use gdrive_core::InventoryRepository;
use gdrive_db::SqliteInventoryRepository;

#[test]
fn unshare_dry_run_shows_actionable_and_non_actionable_rows() {
    let temp_dir = tempfile::TempDir::new().expect("tempdir");
    let login = support::run_mock_command(&temp_dir, &["auth", "login"]);
    assert!(login.status.success(), "stderr: {}", support::stderr(&login));
    let sync = support::run_mock_command(&temp_dir, &["sync"]);
    assert!(sync.status.success(), "stderr: {}", support::stderr(&sync));

    let output = support::run_mock_command(&temp_dir, &["unshare", "--shared"]);
    assert!(output.status.success(), "stderr: {}", support::stderr(&output));

    let stdout = support::stdout(&output);
    assert!(stdout.contains("clearance revocation preview:"));
    assert!(stdout.contains("public-file"));
    assert!(stdout.contains("reason=actionable"));
    assert!(stdout.contains("inherited-file"));
    assert!(stdout.contains("reason=inherited_permission"));

    let explicit_dry_run =
        support::run_mock_command(&temp_dir, &["unshare", "--shared-with", "anyone", "--dry-run"]);
    assert!(explicit_dry_run.status.success(), "stderr: {}", support::stderr(&explicit_dry_run));
    assert!(support::stdout(&explicit_dry_run).contains("clearance revocation preview:"));
}

#[test]
fn unshare_apply_updates_scope_audit_and_follow_up_queries() {
    let temp_dir = tempfile::TempDir::new().expect("tempdir");
    let login = support::run_mock_command(&temp_dir, &["auth", "login"]);
    assert!(login.status.success(), "stderr: {}", support::stderr(&login));
    let sync = support::run_mock_command(&temp_dir, &["sync"]);
    assert!(sync.status.success(), "stderr: {}", support::stderr(&sync));

    let output = support::run_mock_command(
        &temp_dir,
        &["unshare", "--shared-with", "anyone", "--apply", "--yes"],
    );
    assert!(output.status.success(), "stderr: {}", support::stderr(&output));
    let stdout = support::stdout(&output);
    assert!(stdout.contains("pre-mutation release: name=before-unshare-"));
    assert!(stdout.contains("clearance revocations applied: planned=1 applied=1 skipped=0"));
    assert!(stdout.contains("post-apply roll call:"));

    let status = support::run_mock_command(&temp_dir, &["auth", "status"]);
    assert!(status.status.success(), "stderr: {}", support::stderr(&status));
    let stdout = support::stdout(&status);
    assert!(stdout.contains("drive.metadata.readonly"));
    assert!(stdout.contains("drive"));

    let shared =
        support::run_mock_command(&temp_dir, &["find", "shared", "--shared-with", "anyone"]);
    assert!(shared.status.success(), "stderr: {}", support::stderr(&shared));
    assert!(!support::stdout(&shared).contains("public-file"));

    let reports_dir = temp_dir.path().join("reports-after-unshare");
    let report = support::run_mock_command(
        &temp_dir,
        &["report", "sharing", "-o", reports_dir.to_str().expect("reports dir")],
    );
    assert!(report.status.success(), "stderr: {}", support::stderr(&report));
    let sharing_report =
        fs::read_to_string(reports_dir.join("sharing.md")).expect("sharing report");
    assert!(!sharing_report.contains("public-file"));

    let repository =
        SqliteInventoryRepository::new(support::temp_db_path(&temp_dir)).expect("repo");
    let audit = repository.load_audit_log().expect("audit log");
    assert_eq!(audit.len(), 2);
    assert_eq!(audit[0].action, "delete_permission_pending");
    assert_eq!(audit[0].file_id, "public-file");
    assert_eq!(audit[0].permission_id, "perm-anyone");
    assert_eq!(audit[1].action, "delete_permission");
    assert_eq!(audit[1].file_id, "public-file");
    assert_eq!(audit[1].permission_id, "perm-anyone");
}

#[test]
fn unshare_retain_copy_backs_up_before_removing_public_share() {
    let temp_dir = tempfile::TempDir::new().expect("tempdir");
    let login = support::run_mock_command(&temp_dir, &["auth", "login"]);
    assert!(login.status.success(), "stderr: {}", support::stderr(&login));
    let sync = support::run_mock_command(&temp_dir, &["sync"]);
    assert!(sync.status.success(), "stderr: {}", support::stderr(&sync));

    let preview = support::run_mock_command(
        &temp_dir,
        &["unshare", "--shared-with", "anyone", "--retain-copy"],
    );
    assert!(preview.status.success(), "stderr: {}", support::stderr(&preview));
    let preview_stdout = support::stdout(&preview);
    assert!(preview_stdout.contains("retain-copy preview: roots=1"));

    let output = support::run_mock_command(
        &temp_dir,
        &["unshare", "--shared-with", "anyone", "--retain-copy", "--apply", "--yes"],
    );
    assert!(output.status.success(), "stderr: {}", support::stderr(&output));
    let stdout = support::stdout(&output);
    assert!(stdout.contains("pre-mutation release: name=before-unshare-"));
    assert!(stdout.contains("retained copy: roots=1 copied_files=1 created_folders=1"));
    assert!(stdout.contains("clearance revocations applied: planned=1 applied=1 skipped=0"));

    let repository =
        SqliteInventoryRepository::new(support::temp_db_path(&temp_dir)).expect("repo");
    let audit = repository.load_audit_log().expect("audit log");
    assert_eq!(audit.len(), 4);
    let actions = audit.iter().map(|entry| entry.action.as_str()).collect::<Vec<_>>();
    assert_eq!(
        actions,
        vec![
            "create_backup_folder",
            "copy_backup_file",
            "delete_permission_pending",
            "delete_permission"
        ]
    );
    assert_eq!(audit[1].source_file_id.as_deref(), Some("public-file"));

    let shared =
        support::run_mock_command(&temp_dir, &["find", "shared", "--shared-with", "anyone"]);
    assert!(shared.status.success(), "stderr: {}", support::stderr(&shared));
    assert!(!support::stdout(&shared).contains("public-file"));
}

#[test]
fn unshare_apply_requires_yes_in_non_interactive_mode() {
    let temp_dir = tempfile::TempDir::new().expect("tempdir");
    let login = support::run_mock_command(&temp_dir, &["auth", "login"]);
    assert!(login.status.success(), "stderr: {}", support::stderr(&login));
    let sync = support::run_mock_command(&temp_dir, &["sync"]);
    assert!(sync.status.success(), "stderr: {}", support::stderr(&sync));

    let output = support::run_mock_command(
        &temp_dir,
        &["--no-interactive", "unshare", "--shared-with", "anyone", "--apply"],
    );
    assert!(!output.status.success());
    assert!(support::stderr(&output).contains("--yes"));
}

#[test]
fn unshare_apply_rejects_conflicting_dry_run_flag() {
    let temp_dir = tempfile::TempDir::new().expect("tempdir");
    let login = support::run_mock_command(&temp_dir, &["auth", "login"]);
    assert!(login.status.success(), "stderr: {}", support::stderr(&login));
    let sync = support::run_mock_command(&temp_dir, &["sync"]);
    assert!(sync.status.success(), "stderr: {}", support::stderr(&sync));

    let output = support::run_mock_command(
        &temp_dir,
        &["unshare", "--shared-with", "anyone", "--dry-run", "--apply", "--yes"],
    );
    assert!(!output.status.success());
    assert!(support::stderr(&output).contains("cannot be combined"));
}
