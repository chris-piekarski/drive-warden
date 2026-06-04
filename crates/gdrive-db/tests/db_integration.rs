use chrono::Utc;
use gdrive_core::{
    AccountProfile, AuditLogEntry, CreatedFolderEntry, DriveScope, FileRecord, FullSnapshot,
    InventoryRepository, MovedFileEntry, RevokedShareEntry, SyncMode, SyncStats, SyncStatus,
};
use gdrive_db::SqliteInventoryRepository;

#[test]
fn repository_persists_revoked_share_history() {
    let temp_dir = tempfile::TempDir::new().expect("tempdir");
    let repository =
        SqliteInventoryRepository::new(temp_dir.path().join("inventory.db")).expect("repo");

    assert!(repository.load_revoked_shares().expect("empty history").is_empty());

    repository
        .append_revoked_share(&RevokedShareEntry {
            at: Utc::now(),
            command: "unshare".into(),
            file_id: "doc-a".into(),
            file_name: "A.txt".into(),
            file_path: "/Team/A.txt".into(),
            grantee: "user@partner.test".into(),
            grantee_type: "user".into(),
            role: "writer".into(),
            permission_id: "perm-team".into(),
            inherited: true,
            source_folder_id: Some("team-folder".into()),
            revoked_via: "tool".into(),
            note: None,
        })
        .expect("append revoked share");

    let history = repository.load_revoked_shares().expect("history");
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].file_id, "doc-a");
    assert_eq!(history[0].grantee, "user@partner.test");
    assert_eq!(history[0].role, "writer");
    assert!(history[0].inherited);
    assert_eq!(history[0].source_folder_id.as_deref(), Some("team-folder"));
    assert_eq!(history[0].revoked_via, "tool");
}

#[test]
fn repository_persists_moved_file_history() {
    let temp_dir = tempfile::TempDir::new().expect("tempdir");
    let repository =
        SqliteInventoryRepository::new(temp_dir.path().join("inventory.db")).expect("repo");

    assert!(repository.load_moved_files().expect("empty move history").is_empty());

    repository
        .append_moved_file(&MovedFileEntry {
            at: Utc::now(),
            command: "move".into(),
            status: "applied".into(),
            file_id: "doc-a".into(),
            file_name: "A.txt".into(),
            file_path: "/Docs/A.txt".into(),
            mime_type: "text/plain".into(),
            from_parent_ids: vec!["docs".into()],
            from_path: "/Docs/A.txt".into(),
            to_parent_id: "archive".into(),
            to_path: "/Archive".into(),
            move_via: "tool".into(),
            note: Some("test note".into()),
            moved_via_file_id: None,
            moved_via_path: None,
            explicitly_requested: true,
        })
        .expect("append moved file");

    let history = repository.load_moved_files().expect("move history");
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].file_id, "doc-a");
    assert_eq!(history[0].status, "applied");
    assert_eq!(history[0].from_parent_ids, ["docs"]);
    assert_eq!(history[0].to_parent_id, "archive");
    assert_eq!(history[0].note.as_deref(), Some("test note"));
}

#[test]
fn repository_persists_sync_state_and_snapshot() {
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
    let summary = repository
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
                        id: "file".into(),
                        name: "Notes.txt".into(),
                        mime_type: "text/plain".into(),
                        parents: vec!["folder".into()],
                        trashed: false,
                        owned_by_me: true,
                        ..FileRecord::default()
                    },
                ],
            },
            "token-1",
            SyncStats { added: 2, updated: 0, removed: 0 },
        )
        .expect("replace snapshot");

    assert_eq!(summary.file_count, 2);
    assert_eq!(summary.committed_page_token, "token-1");

    let state = repository.get_sync_state().expect("sync state").expect("persisted state");
    assert_eq!(state.account.email, "mock@example.com");
    assert_eq!(state.committed_start_page_token.as_deref(), Some("token-1"));
    assert_eq!(state.last_sync_status, SyncStatus::Committed);

    let snapshot = repository.load_snapshot().expect("snapshot");
    assert_eq!(snapshot.files.len(), 2);

    repository
        .append_audit_log(&AuditLogEntry {
            at: Utc::now(),
            command: "unshare".into(),
            action: "delete_permission".into(),
            file_id: "file".into(),
            permission_id: "perm-1".into(),
            target_label: "anyone with link".into(),
            dry_run: false,
            source_file_id: None,
            backup_file_id: None,
        })
        .expect("append audit");
    let audit = repository.load_audit_log().expect("audit");
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0].permission_id, "perm-1");
}

#[test]
fn repository_persists_created_folder_history() {
    let temp_dir = tempfile::TempDir::new().expect("tempdir");
    let repository =
        SqliteInventoryRepository::new(temp_dir.path().join("inventory.db")).expect("repo");

    assert!(repository.load_created_folders().expect("empty folder history").is_empty());

    repository
        .append_created_folder(&CreatedFolderEntry {
            at: Utc::now(),
            command: "move".into(),
            status: "applied".into(),
            folder_id: "new-folder".into(),
            folder_name: "New".into(),
            folder_path: "/Archive/New".into(),
            parent_id: "archive".into(),
            parent_path: "/Archive".into(),
            provision_path: "/Archive/New".into(),
            create_via: "tool".into(),
            note: None,
        })
        .expect("append created folder");

    let history = repository.load_created_folders().expect("folder history");
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].folder_id, "new-folder");
    assert_eq!(history[0].provision_path, "/Archive/New");
}
