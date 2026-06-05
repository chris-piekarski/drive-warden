mod support;
use std::fs;

use gdrive_core::InventoryRepository;
use gdrive_db::SqliteInventoryRepository;

#[test]
fn first_time_mock_operator_flow_completes() {
    let temp_dir = tempfile::TempDir::new().expect("tempdir");

    let login = support::run_mock_command(&temp_dir, &["auth", "login"]);
    assert!(login.status.success(), "stderr: {}", support::stderr(&login));

    let sync = support::run_mock_command(&temp_dir, &["sync"]);
    assert!(sync.status.success(), "stderr: {}", support::stderr(&sync));
    assert!(support::stdout(&sync).contains("token=start-token-2"));

    let status = support::run_mock_command(&temp_dir, &["auth", "status"]);
    assert!(status.status.success(), "stderr: {}", support::stderr(&status));
    assert!(support::stdout(&status).contains("mock@example.com"));

    let reports_dir = temp_dir.path().join("acceptance-reports");
    let report = support::run_mock_command(
        &temp_dir,
        &["report", "all", "-o", reports_dir.to_str().expect("reports dir")],
    );
    assert!(report.status.success(), "stderr: {}", support::stderr(&report));
    assert!(fs::read_to_string(reports_dir.join("summary.md"))
        .expect("summary report")
        .contains("Warden briefing"));

    let inspect = support::run_mock_command(&temp_dir, &["inspect", "file", "public-file"]);
    assert!(inspect.status.success(), "stderr: {}", support::stderr(&inspect));
    assert!(support::stdout(&inspect).contains("id: public-file"));

    let inspect_exif = support::run_mock_command(&temp_dir, &["inspect", "exif", "photo-file"]);
    assert!(inspect_exif.status.success(), "stderr: {}", support::stderr(&inspect_exif));
    assert!(support::stdout(&inspect_exif).contains("camera_model: Model One"));

    let preview = support::run_mock_command(&temp_dir, &["unshare", "--shared-with", "anyone"]);
    assert!(preview.status.success(), "stderr: {}", support::stderr(&preview));
    assert!(support::stdout(&preview).contains("reason=actionable"));

    let apply = support::run_mock_command(
        &temp_dir,
        &["unshare", "--shared-with", "anyone", "--apply", "--yes"],
    );
    assert!(apply.status.success(), "stderr: {}", support::stderr(&apply));
    assert!(support::stdout(&apply).contains("applied=1"));

    let verify =
        support::run_mock_command(&temp_dir, &["find", "shared", "--shared-with", "anyone"]);
    assert!(verify.status.success(), "stderr: {}", support::stderr(&verify));
    assert!(!support::stdout(&verify).contains("public-file"));

    let logout = support::run_mock_command(&temp_dir, &["auth", "logout"]);
    assert!(logout.status.success(), "stderr: {}", support::stderr(&logout));
    assert!(support::stdout(&logout).contains("Warden credentials cleared."));
}

#[test]
fn revoked_token_requires_relogin_guidance() {
    let temp_dir = tempfile::TempDir::new().expect("tempdir");

    let login = support::run_mock_command(&temp_dir, &["auth", "login"]);
    assert!(login.status.success(), "stderr: {}", support::stderr(&login));

    let status = support::run_mock_command_with_fixture(
        &temp_dir,
        "tests/fixtures/drive_failures/revoked_token",
        &["auth", "status"],
    );
    assert!(!status.status.success());
    assert!(support::stderr(&status).contains("revoked"));
    assert!(support::stderr(&status).contains("auth login"));

    let sync = support::run_mock_command_with_fixture(
        &temp_dir,
        "tests/fixtures/drive_failures/revoked_token",
        &["sync"],
    );
    assert!(!sync.status.success());
    assert!(support::stderr(&sync).contains("auth login"));
}

#[test]
fn invalid_page_token_auto_recovers_with_full_resync() {
    let temp_dir = tempfile::TempDir::new().expect("tempdir");

    let login = support::run_mock_command(&temp_dir, &["auth", "login"]);
    assert!(login.status.success(), "stderr: {}", support::stderr(&login));
    let sync = support::run_mock_command(&temp_dir, &["sync"]);
    assert!(sync.status.success(), "stderr: {}", support::stderr(&sync));

    let recovered_sync = support::run_mock_command_with_fixture(
        &temp_dir,
        "tests/fixtures/drive_failures/invalid_page_token",
        &["sync"],
    );
    assert!(recovered_sync.status.success(), "stderr: {}", support::stderr(&recovered_sync));
    assert!(support::stdout(&recovered_sync).contains("mode=full"));
}

#[test]
fn interrupted_sync_preserves_committed_snapshot_until_recovery() {
    let temp_dir = tempfile::TempDir::new().expect("tempdir");

    let login = support::run_mock_command(&temp_dir, &["auth", "login"]);
    assert!(login.status.success(), "stderr: {}", support::stderr(&login));
    let sync = support::run_mock_command(&temp_dir, &["sync"]);
    assert!(sync.status.success(), "stderr: {}", support::stderr(&sync));

    let failing_sync = support::run_mock_command_with_fixture(
        &temp_dir,
        "tests/fixtures/drive_failures/interrupted_sync",
        &["sync"],
    );
    assert!(!failing_sync.status.success());
    assert!(support::stderr(&failing_sync).contains("interruption"));

    let repository =
        SqliteInventoryRepository::new(support::temp_db_path(&temp_dir)).expect("repository");
    let state = repository.get_sync_state().expect("sync state").expect("persisted state");
    assert_eq!(state.committed_start_page_token.as_deref(), Some("start-token-2"));
    assert_eq!(repository.load_snapshot().expect("snapshot").files.len(), 12);

    let recovered = support::run_mock_command(&temp_dir, &["sync"]);
    assert!(recovered.status.success(), "stderr: {}", support::stderr(&recovered));
    assert!(support::stdout(&recovered).contains("token=start-token-3"));
}

#[test]
fn non_interactive_json_output_stays_stable_for_automation() {
    let temp_dir = tempfile::TempDir::new().expect("tempdir");

    let login = support::run_mock_command(&temp_dir, &["auth", "login"]);
    assert!(login.status.success(), "stderr: {}", support::stderr(&login));
    let sync = support::run_mock_command(&temp_dir, &["sync"]);
    assert!(sync.status.success(), "stderr: {}", support::stderr(&sync));

    let output = support::run_mock_command(
        &temp_dir,
        &["--no-interactive", "--format", "json", "find", "shared", "--shared"],
    );
    assert!(output.status.success(), "stderr: {}", support::stderr(&output));
    let stdout = support::stdout(&output);
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).expect("valid json output");
    assert!(parsed.is_array());
    assert!(!stdout.contains('\u{1b}'));
}
