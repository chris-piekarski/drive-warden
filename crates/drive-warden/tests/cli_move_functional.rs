mod support;

use gdrive_core::InventoryRepository;
use gdrive_db::SqliteInventoryRepository;

#[test]
fn move_preview_and_invalid_destination_are_read_only() {
    let temp_dir = tempfile::TempDir::new().expect("tempdir");
    let login = support::run_mock_command_with_fixture(
        &temp_dir,
        "tests/fixtures/drive_move",
        &["auth", "login"],
    );
    assert!(login.status.success(), "stderr: {}", support::stderr(&login));
    let sync =
        support::run_mock_command_with_fixture(&temp_dir, "tests/fixtures/drive_move", &["sync"]);
    assert!(sync.status.success(), "stderr: {}", support::stderr(&sync));

    let preview = support::run_mock_command_with_fixture(
        &temp_dir,
        "tests/fixtures/drive_move",
        &["move", "--file-id", "loose-file", "--to-path", "/Archive"],
    );
    assert!(preview.status.success(), "stderr: {}", support::stderr(&preview));
    let stdout = support::stdout(&preview);
    assert!(stdout.contains("cell transfer preview:"));
    assert!(stdout.contains("loose-file"));
    assert!(stdout.contains("reason=actionable"));
    assert!(stdout.contains("destination=/Archive"));

    let invalid = support::run_mock_command_with_fixture(
        &temp_dir,
        "tests/fixtures/drive_move",
        &["move", "--file-id", "loose-file", "--to-path", "/Missing"],
    );
    assert!(!invalid.status.success());
    assert!(support::stderr(&invalid).contains("destination folder path `/Missing` was not found"));
    assert!(support::stderr(&invalid).contains("--provision-missing"));
}

#[test]
fn move_guards_non_interactive_and_conflicting_flags() {
    let temp_dir = tempfile::TempDir::new().expect("tempdir");
    let login = support::run_mock_command_with_fixture(
        &temp_dir,
        "tests/fixtures/drive_move",
        &["auth", "login"],
    );
    assert!(login.status.success(), "stderr: {}", support::stderr(&login));
    let sync =
        support::run_mock_command_with_fixture(&temp_dir, "tests/fixtures/drive_move", &["sync"]);
    assert!(sync.status.success(), "stderr: {}", support::stderr(&sync));

    let missing_yes = support::run_mock_command_with_fixture(
        &temp_dir,
        "tests/fixtures/drive_move",
        &[
            "--no-interactive",
            "move",
            "--file-id",
            "loose-file",
            "--to-path",
            "/Archive",
            "--apply",
        ],
    );
    assert!(!missing_yes.status.success());
    assert!(support::stderr(&missing_yes).contains("--yes"));

    let conflict = support::run_mock_command_with_fixture(
        &temp_dir,
        "tests/fixtures/drive_move",
        &[
            "move",
            "--file-id",
            "loose-file",
            "--to-path",
            "/Archive",
            "--dry-run",
            "--apply",
            "--yes",
        ],
    );
    assert!(!conflict.status.success());
    assert!(support::stderr(&conflict).contains("cannot be combined"));
}

#[test]
fn move_apply_reparents_file_and_records_history() {
    let temp_dir = tempfile::TempDir::new().expect("tempdir");
    let login = support::run_mock_command_with_fixture(
        &temp_dir,
        "tests/fixtures/drive_move",
        &["auth", "login"],
    );
    assert!(login.status.success(), "stderr: {}", support::stderr(&login));
    let sync =
        support::run_mock_command_with_fixture(&temp_dir, "tests/fixtures/drive_move", &["sync"]);
    assert!(sync.status.success(), "stderr: {}", support::stderr(&sync));

    let apply = support::run_mock_command_with_fixture(
        &temp_dir,
        "tests/fixtures/drive_move",
        &[
            "move",
            "--file-id",
            "loose-file",
            "--to-folder-id",
            "archive-folder",
            "--apply",
            "--yes",
        ],
    );
    assert!(apply.status.success(), "stderr: {}", support::stderr(&apply));
    let stdout = support::stdout(&apply);
    assert!(stdout.contains("pre-mutation release: name=before-move-"));
    assert!(stdout.contains("cell transfers applied: planned=1 applied=1 skipped=0"));
    assert!(stdout.contains("post-apply roll call:"));

    let repository =
        SqliteInventoryRepository::new(support::temp_db_path(&temp_dir)).expect("repo");
    let snapshot = repository.load_snapshot().expect("snapshot");
    let moved = snapshot.files.iter().find(|file| file.id == "loose-file").expect("loose file");
    assert_eq!(moved.parents, ["archive-folder"]);
    let audit = repository.load_audit_log().expect("audit log");
    assert_eq!(audit.len(), 2);
    assert_eq!(audit[0].action, "move_file_pending");
    assert_eq!(audit[1].action, "move_file");
    let history = repository.load_moved_files().expect("move history");
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].status, "pending");
    assert_eq!(history[1].status, "applied");
    assert_eq!(history[1].from_path, "/Docs/Loose.txt");
    assert_eq!(history[1].to_parent_id, "archive-folder");
}

#[test]
fn move_apply_reparents_folder_without_reparenting_children() {
    let temp_dir = tempfile::TempDir::new().expect("tempdir");
    let login = support::run_mock_command_with_fixture(
        &temp_dir,
        "tests/fixtures/drive_move",
        &["auth", "login"],
    );
    assert!(login.status.success(), "stderr: {}", support::stderr(&login));
    let sync =
        support::run_mock_command_with_fixture(&temp_dir, "tests/fixtures/drive_move", &["sync"]);
    assert!(sync.status.success(), "stderr: {}", support::stderr(&sync));

    let apply = support::run_mock_command_with_fixture(
        &temp_dir,
        "tests/fixtures/drive_move",
        &["move", "--path", "/Docs/Project", "--to-path", "/Archive", "--apply", "--yes"],
    );
    assert!(apply.status.success(), "stderr: {}", support::stderr(&apply));
    assert!(
        support::stdout(&apply).contains("cell transfers applied: planned=1 applied=1 skipped=0")
    );

    let repository =
        SqliteInventoryRepository::new(support::temp_db_path(&temp_dir)).expect("repo");
    let snapshot = repository.load_snapshot().expect("snapshot");
    let folder = snapshot.files.iter().find(|file| file.id == "source-folder").expect("folder");
    assert_eq!(folder.parents, ["archive-folder"]);
    let child = snapshot.files.iter().find(|file| file.id == "nested-file").expect("child");
    assert_eq!(child.parents, ["source-folder"]);
    let items = repository.load_inventory_items().expect("inventory");
    let child_item = items.iter().find(|item| item.file.id == "nested-file").expect("child item");
    assert_eq!(child_item.path.primary_path, "/Archive/Project/Plan.txt");
}

#[test]
fn move_preview_supports_root_and_provisioning() {
    let temp_dir = tempfile::TempDir::new().expect("tempdir");
    let login = support::run_mock_command_with_fixture(
        &temp_dir,
        "tests/fixtures/drive_move",
        &["auth", "login"],
    );
    assert!(login.status.success(), "stderr: {}", support::stderr(&login));
    let sync =
        support::run_mock_command_with_fixture(&temp_dir, "tests/fixtures/drive_move", &["sync"]);
    assert!(sync.status.success(), "stderr: {}", support::stderr(&sync));

    let root_preview = support::run_mock_command_with_fixture(
        &temp_dir,
        "tests/fixtures/drive_move",
        &["move", "--file-id", "loose-file", "--to-root"],
    );
    assert!(root_preview.status.success(), "stderr: {}", support::stderr(&root_preview));
    assert!(support::stdout(&root_preview).contains("destination=/"));

    let provision_preview = support::run_mock_command_with_fixture(
        &temp_dir,
        "tests/fixtures/drive_move",
        &[
            "move",
            "--file-id",
            "loose-file",
            "--to-path",
            "/Archive/NewShelf",
            "--provision-missing",
        ],
    );
    assert!(provision_preview.status.success(), "stderr: {}", support::stderr(&provision_preview));
    let stdout = support::stdout(&provision_preview);
    assert!(stdout.contains("destination provisioning:"));
    assert!(stdout.contains("/Archive/NewShelf"));
}

#[test]
fn move_apply_provisions_destination_and_records_folder_history() {
    let temp_dir = tempfile::TempDir::new().expect("tempdir");
    let login = support::run_mock_command_with_fixture(
        &temp_dir,
        "tests/fixtures/drive_move",
        &["auth", "login"],
    );
    assert!(login.status.success(), "stderr: {}", support::stderr(&login));
    let sync =
        support::run_mock_command_with_fixture(&temp_dir, "tests/fixtures/drive_move", &["sync"]);
    assert!(sync.status.success(), "stderr: {}", support::stderr(&sync));

    let apply = support::run_mock_command_with_fixture(
        &temp_dir,
        "tests/fixtures/drive_move",
        &[
            "move",
            "--file-id",
            "loose-file",
            "--to-path",
            "/Archive/NewShelf",
            "--provision-missing",
            "--apply",
            "--yes",
        ],
    );
    assert!(apply.status.success(), "stderr: {}", support::stderr(&apply));
    assert!(support::stdout(&apply).contains("destination provisioning: created=1"));

    let repository =
        SqliteInventoryRepository::new(support::temp_db_path(&temp_dir)).expect("repo");
    let created = repository.load_created_folders().expect("created folders");
    assert_eq!(created.len(), 2);
    assert_eq!(created[0].status, "pending");
    assert_eq!(created[1].status, "applied");
    assert_eq!(created[1].provision_path, "/Archive/NewShelf");
}
