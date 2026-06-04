mod support;

#[test]
fn sync_bootstrap_then_delta_against_mock_backend() {
    let temp_dir = tempfile::TempDir::new().expect("tempdir");
    let login = support::run_mock_command(&temp_dir, &["auth", "login"]);
    assert!(login.status.success(), "stderr: {}", support::stderr(&login));

    let first_sync = support::run_mock_command(&temp_dir, &["sync"]);
    assert!(first_sync.status.success(), "stderr: {}", support::stderr(&first_sync));
    let stdout = support::stdout(&first_sync);
    assert!(stdout.contains("mode=full"));
    assert!(stdout.contains("added=12"));
    assert!(stdout.contains("files=12"));
    assert!(stdout.contains("paths=12"));
    assert!(stdout.contains("token=start-token-2"));

    let second_sync = support::run_mock_command(&temp_dir, &["sync"]);
    assert!(second_sync.status.success(), "stderr: {}", support::stderr(&second_sync));
    let stdout = support::stdout(&second_sync);
    assert!(stdout.contains("mode=delta"));
    assert!(stdout.contains("added=0"));
    assert!(stdout.contains("updated=0"));
    assert!(stdout.contains("removed=0"));
    assert!(stdout.contains("token=start-token-3"));
}
