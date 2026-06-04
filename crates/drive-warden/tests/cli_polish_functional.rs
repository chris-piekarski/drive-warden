mod support;

use std::process::Command;

#[test]
fn completions_render_for_supported_shells() {
    let temp_dir = tempfile::TempDir::new().expect("tempdir");

    for shell in ["bash", "zsh", "fish"] {
        let output = support::run_mock_command(&temp_dir, &["completions", shell]);
        assert!(output.status.success(), "shell={shell} stderr={}", support::stderr(&output));
        let stdout = support::stdout(&output);
        assert!(stdout.contains("drive-warden"), "shell={shell} stdout={stdout}");
    }
}

#[test]
fn inspect_exif_upgrades_scope_and_db_commands_are_available() {
    let temp_dir = tempfile::TempDir::new().expect("tempdir");

    let login = support::run_mock_command(&temp_dir, &["auth", "login"]);
    assert!(login.status.success(), "stderr: {}", support::stderr(&login));

    let inspect_exif = support::run_mock_command(&temp_dir, &["inspect", "exif", "photo-file"]);
    assert!(inspect_exif.status.success(), "stderr: {}", support::stderr(&inspect_exif));
    let stdout = support::stdout(&inspect_exif);
    assert!(stdout.contains("width: 4032"));
    assert!(stdout.contains("iso_speed: 200"));

    let status = support::run_mock_command(&temp_dir, &["auth", "status"]);
    assert!(status.status.success(), "stderr: {}", support::stderr(&status));
    assert!(support::stdout(&status).contains("drive.readonly"));

    let sync = support::run_mock_command(&temp_dir, &["sync"]);
    assert!(sync.status.success(), "stderr: {}", support::stderr(&sync));

    let stats = support::run_mock_command(&temp_dir, &["db", "stats"]);
    assert!(stats.status.success(), "stderr: {}", support::stderr(&stats));
    let stdout = support::stdout(&stats);
    assert!(stdout.contains("files: 12"));
    assert!(stdout.contains("committed_page_token: start-token-2"));

    let vacuum = support::run_mock_command(&temp_dir, &["db", "vacuum"]);
    assert!(vacuum.status.success(), "stderr: {}", support::stderr(&vacuum));
    assert!(support::stdout(&vacuum).contains("vacuum complete:"));
}

#[test]
fn google_backend_uses_configured_live_paths_and_fails_without_credentials() {
    let temp_dir = tempfile::TempDir::new().expect("tempdir");
    let config_path = temp_dir.path().join("google.toml");
    let credentials_path = temp_dir.path().join("custom-credentials.json");
    let token_path = temp_dir.path().join("custom-tokens.json");
    let session_path = temp_dir.path().join("custom-session.json");
    std::fs::write(
        &config_path,
        format!(
            r#"[backend]
kind = "google"

[google]
credentials_path = "{}"
token_path = "{}"
session_path = "{}"
"#,
            credentials_path.display(),
            token_path.display(),
            session_path.display(),
        ),
    )
    .expect("config");

    let output = Command::new(env!("CARGO_BIN_EXE_drive-warden"))
        .current_dir(support::workspace_root())
        .args([
            "--config",
            config_path.to_str().expect("config path"),
            "--db",
            support::temp_db_path(&temp_dir).to_str().expect("db path"),
            "auth",
            "login",
        ])
        .output()
        .expect("run drive-warden");

    assert!(!output.status.success(), "stdout={}", support::stdout(&output));
    let stderr = support::stderr(&output);
    assert!(stderr.contains(&credentials_path.display().to_string()), "stderr={stderr}");
    assert!(!token_path.exists(), "token path should stay untouched");
    assert!(!session_path.exists(), "session path should stay untouched");
}
