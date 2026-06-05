mod support;
use std::fs;

#[test]
fn mock_auth_login_status_logout_round_trip() {
    let temp_dir = tempfile::TempDir::new().expect("tempdir");

    let initial_status = support::run_mock_command(&temp_dir, &["auth", "status"]);
    assert!(initial_status.status.success());
    assert!(support::stdout(&initial_status).contains("Warden off duty"));

    let login = support::run_mock_command(&temp_dir, &["auth", "login"]);
    assert!(login.status.success(), "stderr: {}", support::stderr(&login));
    assert!(support::stdout(&login).contains("Warden credentials confirmed for mock@example.com"));
    assert!(support::mock_auth_path(&temp_dir).exists());

    let status = support::run_mock_command(&temp_dir, &["auth", "status"]);
    assert!(status.status.success(), "stderr: {}", support::stderr(&status));
    let stdout = support::stdout(&status);
    assert!(stdout.contains("Warden on duty: mock@example.com"));
    assert!(stdout.contains("drive.metadata.readonly"));

    let logout = support::run_mock_command(&temp_dir, &["auth", "logout"]);
    assert!(logout.status.success(), "stderr: {}", support::stderr(&logout));
    assert!(support::stdout(&logout).contains("Warden credentials cleared."));
    assert!(!support::mock_auth_path(&temp_dir).exists());

    let final_status = support::run_mock_command(&temp_dir, &["auth", "status"]);
    assert!(final_status.status.success());
    assert!(support::stdout(&final_status).contains("Warden off duty"));
}

#[test]
fn mock_sync_requires_login() {
    let temp_dir = tempfile::TempDir::new().expect("tempdir");
    let output = support::run_mock_command(&temp_dir, &["sync"]);

    assert!(!output.status.success());
    let stderr = support::stderr(&output);
    assert!(stderr.contains("not logged in"));
    assert!(stderr.contains("auth login"));
}

#[test]
fn report_all_writes_actionable_markdown_pack() {
    let temp_dir = tempfile::TempDir::new().expect("tempdir");
    let login = support::run_mock_command(&temp_dir, &["auth", "login"]);
    assert!(login.status.success(), "stderr: {}", support::stderr(&login));
    let sync = support::run_mock_command(&temp_dir, &["sync"]);
    assert!(sync.status.success(), "stderr: {}", support::stderr(&sync));

    let reports_dir = temp_dir.path().join("reports-out");
    let output = support::run_mock_command(
        &temp_dir,
        &["report", "all", "-o", reports_dir.to_str().expect("reports dir")],
    );
    assert!(output.status.success(), "stderr: {}", support::stderr(&output));

    let summary = fs::read_to_string(reports_dir.join("summary.md")).expect("summary report");
    let duplicates =
        fs::read_to_string(reports_dir.join("duplicates.md")).expect("duplicates report");
    let sharing = fs::read_to_string(reports_dir.join("sharing.md")).expect("sharing report");
    let storage = fs::read_to_string(reports_dir.join("storage.md")).expect("storage report");

    assert!(summary.contains("Identity collision groups"));
    assert!(duplicates.contains("md5-archive-group"));
    assert!(duplicates.contains("Archive Copy.zip"));
    assert!(sharing.contains("anyone with link"));
    assert!(sharing.contains("vendor@outside.test"));
    assert!(storage.contains("10485760"));
    assert!(storage.contains("Idle inmate rows"));
}
