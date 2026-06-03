mod support;

use std::fs;

#[test]
fn db_remote_sync_pushes_then_refuses_ambiguous_overwrite_and_pulls_when_local_missing() {
    let temp_dir = tempfile::TempDir::new().expect("tempdir");
    let login = support::run_mock_command(&temp_dir, &["auth", "login"]);
    assert!(login.status.success(), "stderr: {}", support::stderr(&login));
    let sync = support::run_mock_command(&temp_dir, &["sync"]);
    assert!(sync.status.success(), "stderr: {}", support::stderr(&sync));

    let push = support::run_mock_command(&temp_dir, &["db", "remote", "sync"]);
    assert!(push.status.success(), "stderr: {}", support::stderr(&push));
    let stdout = support::stdout(&push);
    assert!(stdout.contains("remote db pushed:"));
    assert!(stdout.contains("manifest: sha256="));

    let status = support::run_mock_command(&temp_dir, &["db", "remote", "status"]);
    assert!(status.status.success(), "stderr: {}", support::stderr(&status));
    let stdout = support::stdout(&status);
    assert!(stdout.contains("decision=needs_explicit_direction"));
    assert!(stdout.contains("local_db_instance_id:"));
    assert!(stdout.contains("local_remote_generation: 1"));
    assert!(stdout.contains("remote_db_instance_id:"));
    assert!(stdout.contains("remote_db_generation: 1"));

    let ambiguous = support::run_mock_command(&temp_dir, &["db", "remote", "sync"]);
    assert!(!ambiguous.status.success());
    assert!(support::stderr(&ambiguous)
        .contains("choose `db remote push --yes` or `db remote pull --yes`"));

    fs::remove_file(support::temp_db_path(&temp_dir)).expect("remove local db");
    let pull = support::run_mock_command(&temp_dir, &["db", "remote", "sync"]);
    assert!(pull.status.success(), "stderr: {}", support::stderr(&pull));
    assert!(support::stdout(&pull).contains("remote db pulled:"));
    assert!(support::temp_db_path(&temp_dir).exists());

    let status_after_pull = support::run_mock_command(&temp_dir, &["db", "remote", "status"]);
    assert!(status_after_pull.status.success(), "stderr: {}", support::stderr(&status_after_pull));
    assert!(support::stdout(&status_after_pull).contains("local_remote_generation: 1"));
}

#[test]
fn db_remote_release_creates_named_non_overwriting_snapshot() {
    let temp_dir = tempfile::TempDir::new().expect("tempdir");
    let login = support::run_mock_command(&temp_dir, &["auth", "login"]);
    assert!(login.status.success(), "stderr: {}", support::stderr(&login));
    let sync = support::run_mock_command(&temp_dir, &["sync"]);
    assert!(sync.status.success(), "stderr: {}", support::stderr(&sync));
    let push = support::run_mock_command(&temp_dir, &["db", "remote", "push", "--yes"]);
    assert!(push.status.success(), "stderr: {}", support::stderr(&push));

    let release = support::run_mock_command(
        &temp_dir,
        &["db", "remote", "release", "--name", "coors-trash-v1", "--yes"],
    );
    assert!(release.status.success(), "stderr: {}", support::stderr(&release));
    let stdout = support::stdout(&release);
    assert!(stdout.contains("remote db release created: name=coors-trash-v1"));
    assert!(stdout.contains("manifest: sha256="));

    let list = support::run_mock_command(&temp_dir, &["db", "remote", "release", "list"]);
    assert!(list.status.success(), "stderr: {}", support::stderr(&list));
    let stdout = support::stdout(&list);
    assert!(stdout.contains("remote db releases: count=1"));
    assert!(stdout.contains("coors-trash-v1"));

    let duplicate = support::run_mock_command(
        &temp_dir,
        &["db", "remote", "release", "--name", "coors-trash-v1", "--yes"],
    );
    assert!(!duplicate.status.success());
    assert!(support::stderr(&duplicate).contains("already exists"));
}

#[test]
fn db_remote_push_and_pull_require_yes_when_non_interactive() {
    let temp_dir = tempfile::TempDir::new().expect("tempdir");
    let login = support::run_mock_command(&temp_dir, &["auth", "login"]);
    assert!(login.status.success(), "stderr: {}", support::stderr(&login));
    let sync = support::run_mock_command(&temp_dir, &["sync"]);
    assert!(sync.status.success(), "stderr: {}", support::stderr(&sync));

    let missing_yes =
        support::run_mock_command(&temp_dir, &["--no-interactive", "db", "remote", "push"]);
    assert!(!missing_yes.status.success());
    assert!(support::stderr(&missing_yes).contains("--yes"));

    let push = support::run_mock_command(
        &temp_dir,
        &["--no-interactive", "db", "remote", "push", "--yes"],
    );
    assert!(push.status.success(), "stderr: {}", support::stderr(&push));

    let missing_pull_yes =
        support::run_mock_command(&temp_dir, &["--no-interactive", "db", "remote", "pull"]);
    assert!(!missing_pull_yes.status.success());
    assert!(support::stderr(&missing_pull_yes).contains("--yes"));
}
