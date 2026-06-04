mod support;

use gdrive_core::{InventoryRepository, PathState};
use gdrive_db::SqliteInventoryRepository;

#[test]
fn sync_populates_sqlite_state_and_paths() {
    let temp_dir = tempfile::TempDir::new().expect("tempdir");
    let login = support::run_mock_command(&temp_dir, &["auth", "login"]);
    assert!(login.status.success(), "stderr: {}", support::stderr(&login));

    let sync = support::run_mock_command(&temp_dir, &["sync"]);
    assert!(sync.status.success(), "stderr: {}", support::stderr(&sync));

    let repository =
        SqliteInventoryRepository::new(support::temp_db_path(&temp_dir)).expect("repo");
    let state = repository.get_sync_state().expect("sync state").expect("persisted sync state");
    assert_eq!(state.account.email, "mock@example.com");
    assert_eq!(state.committed_start_page_token.as_deref(), Some("start-token-2"));

    let snapshot = repository.load_snapshot().expect("snapshot");
    assert_eq!(snapshot.files.len(), 12);
    assert!(snapshot.files.iter().any(|file| file.id == "duplicate-b"));
    assert!(!snapshot.files.iter().any(|file| file.id == "old-report"));

    let orphan_path = repository
        .lookup_path_entry("orphan-file")
        .expect("orphan path")
        .expect("orphan path exists");
    assert_eq!(orphan_path.path_state, PathState::Orphaned);
    assert!(orphan_path.primary_path.contains("Loose.txt"));

    let multi_parent = repository
        .lookup_path_entry("shared-shortcut")
        .expect("multi-parent path")
        .expect("multi-parent path exists");
    assert_eq!(multi_parent.path_state, PathState::MultiParent);
    assert_eq!(multi_parent.all_paths.len(), 2);
}

#[test]
fn find_and_inspect_commands_query_local_snapshot() {
    let temp_dir = tempfile::TempDir::new().expect("tempdir");
    let login = support::run_mock_command(&temp_dir, &["auth", "login"]);
    assert!(login.status.success(), "stderr: {}", support::stderr(&login));
    let sync = support::run_mock_command(&temp_dir, &["sync"]);
    assert!(sync.status.success(), "stderr: {}", support::stderr(&sync));

    let duplicates = support::run_mock_command(&temp_dir, &["find", "duplicates"]);
    assert!(duplicates.status.success(), "stderr: {}", support::stderr(&duplicates));
    let stdout = support::stdout(&duplicates);
    assert!(stdout.contains("group md5-archive-group"));
    assert!(stdout.contains("duplicate-a"));
    assert!(stdout.contains("duplicate-b"));

    let shared =
        support::run_mock_command(&temp_dir, &["find", "shared", "--shared-with", "anyone"]);
    assert!(shared.status.success(), "stderr: {}", support::stderr(&shared));
    let stdout = support::stdout(&shared);
    assert!(stdout.contains("public-file"));
    assert!(stdout.contains("anyone"));

    let large = support::run_mock_command(&temp_dir, &["find", "large", "--min", "5000"]);
    assert!(large.status.success(), "stderr: {}", support::stderr(&large));
    let stdout = support::stdout(&large);
    assert!(stdout.contains("duplicate-a"));
    assert!(stdout.contains("10485760"));

    let inspect = support::run_mock_command(&temp_dir, &["inspect", "file", "public-file"]);
    assert!(inspect.status.success(), "stderr: {}", support::stderr(&inspect));
    let stdout = support::stdout(&inspect);
    assert!(stdout.contains("id: public-file"));
    assert!(stdout.contains("path: /Docs/PublicDeck.pdf"));
    assert!(stdout.contains("sharing_findings: 1"));

    let inspect_exif = support::run_mock_command(&temp_dir, &["inspect", "exif", "photo-file"]);
    assert!(inspect_exif.status.success(), "stderr: {}", support::stderr(&inspect_exif));
    let stdout = support::stdout(&inspect_exif);
    assert!(stdout.contains("id: photo-file"));
    assert!(stdout.contains("source: drive_image_media_metadata"));
    assert!(stdout.contains("camera_model: Model One"));
}
