use gdrive_core::{
    AccountProfile, DriveScope, FileRecord, FullSnapshot, InventoryRepository, PathState, SyncMode,
    SyncStats,
};
use gdrive_db::SqliteInventoryRepository;

#[test]
fn repository_rebuilds_orphan_and_multi_parent_paths() {
    let temp_dir = tempfile::TempDir::new().expect("tempdir");
    let repository =
        SqliteInventoryRepository::new(temp_dir.path().join("inventory.db")).expect("repo");
    let account = AccountProfile {
        account_id: "account-1".into(),
        email: "mock@example.com".into(),
        display_name: Some("Mock".into()),
    };
    let run = repository
        .begin_sync_run(&account, &[DriveScope::MetadataReadonly], SyncMode::Full, None)
        .expect("sync run");

    repository
        .replace_snapshot(
            &run,
            &account,
            &[DriveScope::MetadataReadonly],
            &FullSnapshot {
                files: vec![
                    FileRecord {
                        id: "folder".into(),
                        name: "Docs".into(),
                        mime_type: "application/vnd.google-apps.folder".into(),
                        parents: vec!["root".into()],
                        trashed: false,
                        owned_by_me: true,
                        ..FileRecord::default()
                    },
                    FileRecord {
                        id: "multi".into(),
                        name: "Shortcut.note".into(),
                        mime_type: "application/octet-stream".into(),
                        parents: vec!["root".into(), "folder".into()],
                        trashed: false,
                        owned_by_me: false,
                        ..FileRecord::default()
                    },
                    FileRecord {
                        id: "orphan".into(),
                        name: "Loose.txt".into(),
                        mime_type: "text/plain".into(),
                        parents: vec!["missing".into()],
                        trashed: false,
                        owned_by_me: true,
                        ..FileRecord::default()
                    },
                ],
            },
            "token-1",
            SyncStats { added: 3, updated: 0, removed: 0 },
        )
        .expect("replace snapshot");

    let multi = repository.lookup_path_entry("multi").expect("multi lookup").expect("multi path");
    assert_eq!(multi.path_state, PathState::MultiParent);
    assert_eq!(multi.all_paths.len(), 2);

    let orphan =
        repository.lookup_path_entry("orphan").expect("orphan lookup").expect("orphan path");
    assert_eq!(orphan.path_state, PathState::Orphaned);
    assert!(orphan.primary_path.contains("Loose.txt"));
}
