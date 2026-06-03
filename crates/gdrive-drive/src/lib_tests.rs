use std::path::PathBuf;

use gdrive_core::PermissionRecord;
use serde_json::json;
use tempfile::TempDir;

use crate::google_live::{
    copy_file_request, create_folder_request, drive_find_file_query, drive_folder_listing_query,
    escape_drive_query, filter_remote_files_by_prefix, inspect_exif_details_from_record,
    is_retryable_google_error, retry_delay, trash_file_request, update_file_request,
    upload_file_request,
};

use super::*;

fn write_fixture(temp_dir: &TempDir, contents: &str) -> PathBuf {
    let fixture_dir = temp_dir.path().join("fixture");
    std::fs::create_dir_all(fixture_dir.join("api")).expect("fixture dir");
    std::fs::write(fixture_dir.join("api/mock-drive.json"), contents).expect("fixture file");
    fixture_dir
}

fn sample_fixture() -> &'static str {
    r#"{
  "account": { "account_id": "account-1", "email": "mock@example.com", "display_name": "Mock" },
  "start_page_token": "start-token-1",
  "file_pages": {
    "__start__": {
      "next_page_token": null,
      "files": [
        {
          "id": "photo-file",
          "name": "Photo.jpg",
          "mime_type": "image/jpeg",
          "shared": true,
          "permissions": [{ "id": "perm-1", "permission_type": "user", "email_address": "user@example.com", "actionable": true }],
          "image_media_metadata": { "width": 10, "height": 20 }
        }
      ]
    }
  },
  "change_pages": {
    "start-token-1": {
      "next_page_token": null,
      "new_start_page_token": "start-token-2",
      "removed_file_ids": [],
      "updated_files": []
    }
  }
}"#
}

#[test]
fn google_gateway_requires_real_credentials_before_live_login() {
    let temp_dir = TempDir::new().expect("tempdir");
    let gateway = GoogleDriveGateway::with_paths(
        temp_dir.path().join("missing-credentials.json"),
        temp_dir.path().join("google-tokens.json"),
        temp_dir.path().join("google-session.json"),
    );
    let runtime = tokio::runtime::Runtime::new().expect("runtime");

    let login_error = runtime
        .block_on(gateway.login(DriveScope::MetadataReadonly))
        .expect_err("missing credentials should fail");
    assert!(login_error.to_string().contains("credentials were not found"));
    assert!(!runtime.block_on(gateway.logout()).expect("logout"));
    assert!(runtime.block_on(gateway.auth_status()).expect("status").session.is_none());

    let ensure_scope_error =
        runtime.block_on(gateway.ensure_scope(DriveScope::Drive)).expect_err("scope");
    assert!(ensure_scope_error.to_string().contains("not logged in"));
}

#[test]
fn google_gateway_session_metadata_upgrade_and_legacy_paths_are_covered() {
    let temp_dir = TempDir::new().expect("tempdir");
    let credentials_path = temp_dir.path().join("credentials.json");
    let token_path = temp_dir.path().join("google-tokens.json");
    let session_path = temp_dir.path().join("google-session.json");
    let gateway = GoogleDriveGateway::with_paths(&credentials_path, &token_path, &session_path);
    let runtime = tokio::runtime::Runtime::new().expect("runtime");

    std::fs::write(&token_path, "[]").expect("token cache placeholder");
    gateway
        .write_google_session_state(&GoogleSessionState {
            version: 1,
            account: AccountProfile {
                account_id: "account-1".into(),
                email: "real@example.com".into(),
                display_name: Some("Real".into()),
            },
            active_scopes: vec![DriveScope::MetadataReadonly],
            credentials_path: credentials_path.display().to_string(),
            token_path: token_path.display().to_string(),
        })
        .expect("write state");

    let status = runtime.block_on(gateway.auth_status()).expect("status");
    assert_eq!(status.session.expect("session").account.email, "real@example.com");

    let upgrade_error = runtime
        .block_on(gateway.ensure_scope(DriveScope::DriveReadonly))
        .expect_err("upgrade should need credentials");
    assert!(upgrade_error.to_string().contains("credentials were not found"));
    let persisted = gateway.load_google_session_state().expect("state").expect("session");
    assert_eq!(persisted.active_scopes, vec![DriveScope::MetadataReadonly]);

    std::fs::remove_file(&token_path).expect("remove token cache");
    let missing_token_error = runtime.block_on(gateway.auth_status()).expect_err("status error");
    assert!(missing_token_error.to_string().contains("token cache"));

    write_session_file(
        &session_path,
        &AuthSession {
            account: AccountProfile {
                account_id: "legacy".into(),
                email: "legacy@example.com".into(),
                display_name: None,
            },
            active_scopes: vec![DriveScope::MetadataReadonly],
        },
    )
    .expect("legacy session");
    std::fs::write(&token_path, "[]").expect("token cache placeholder");
    let legacy_error = gateway.load_google_session_state().expect_err("legacy session should fail");
    assert!(legacy_error.to_string().contains("legacy Google session format"));
}

#[test]
fn helper_functions_cover_google_session_and_filesystem_edges() {
    let temp_dir = TempDir::new().expect("tempdir");
    let default_gateway =
        GoogleDriveGateway::new(temp_dir.path().join("profile/google-session.json"));
    assert!(default_gateway.credentials_path.ends_with("profile/credentials.json"));
    assert!(default_gateway.token_path.ends_with("profile/google-tokens.json"));

    let bad_state_path = temp_dir.path().join("bad-session.json");
    let bad_gateway = GoogleDriveGateway::with_paths(
        temp_dir.path().join("credentials.json"),
        temp_dir.path().join("google-tokens.json"),
        &bad_state_path,
    );
    std::fs::write(&bad_gateway.token_path, "[]").expect("token cache");
    std::fs::write(&bad_state_path, "{not-json").expect("bad session");
    let parse_error = bad_gateway.load_google_session_state().expect_err("bad state");
    assert!(parse_error.to_string().contains("failed to parse Google session metadata"));

    let dir_state_path = temp_dir.path().join("session-dir");
    std::fs::create_dir_all(&dir_state_path).expect("dir state");
    let dir_gateway = GoogleDriveGateway::with_paths(
        temp_dir.path().join("credentials-2.json"),
        temp_dir.path().join("google-tokens-2.json"),
        &dir_state_path,
    );
    assert!(dir_gateway.load_google_session_state().is_err());

    let state_gateway = GoogleDriveGateway::with_paths(
        temp_dir.path().join("credentials-3.json"),
        temp_dir.path().join("google-tokens-3.json"),
        temp_dir.path().join("google-session-3.json"),
    );
    std::fs::write(&state_gateway.token_path, "[]").expect("token cache");
    state_gateway
        .write_google_session_state(&GoogleSessionState {
            version: 1,
            account: AccountProfile {
                account_id: "account-3".into(),
                email: "session@example.com".into(),
                display_name: None,
            },
            active_scopes: vec![DriveScope::MetadataReadonly, DriveScope::DriveReadonly],
            credentials_path: state_gateway.credentials_path.display().to_string(),
            token_path: state_gateway.token_path.display().to_string(),
        })
        .expect("write state");
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    let ensured = runtime
        .block_on(state_gateway.ensure_scope_internal(DriveScope::MetadataReadonly))
        .expect("ensure scope");
    assert_eq!(
        ensured.active_scopes,
        vec![DriveScope::MetadataReadonly, DriveScope::DriveReadonly]
    );
    assert_eq!(
        GoogleDriveGateway::highest_scope_from_session(
            state_gateway.load_google_session_state().expect("state").as_ref(),
            DriveScope::MetadataReadonly,
        ),
        DriveScope::DriveReadonly
    );

    let read_dir_error = read_session_file(temp_dir.path()).expect_err("read dir should fail");
    assert!(!read_dir_error.to_string().is_empty());
    ensure_parent_dir(std::path::Path::new("plain-file.json")).expect("no parent dir");

    let permissions_file = temp_dir.path().join("private.json");
    std::fs::write(&permissions_file, "{}").expect("private file");
    ensure_private_file_permissions(&permissions_file).expect("chmod");
    ensure_private_file_permissions(&temp_dir.path().join("missing.json")).expect("missing ok");
    let delete_error = delete_file_if_exists(temp_dir.path()).expect_err("remove dir should fail");
    assert!(!delete_error.to_string().is_empty());
}

#[test]
fn live_mapping_helpers_cover_files_changes_and_tokens() {
    let file: google_drive3::api::File = serde_json::from_value(json!({
        "id": "photo-1",
        "name": "Photo.jpg",
        "mimeType": "image/jpeg",
        "parents": ["root"],
        "ownedByMe": true,
        "shared": true,
        "capabilities": { "canShare": true },
        "size": "2048",
        "md5Checksum": "abcd",
        "permissions": [
            {
                "id": "perm-user",
                "type": "user",
                "role": "writer",
                "emailAddress": "person@example.com",
                "displayName": "Person",
                "permissionDetails": [{ "inherited": false, "permissionType": "file", "role": "writer" }]
            },
            {
                "id": "perm-group",
                "type": "group",
                "role": "reader",
                "emailAddress": "group@example.com",
                "displayName": "Group",
                "permissionDetails": [{ "inherited": false, "permissionType": "file", "role": "reader" }]
            },
            {
                "id": "perm-owner",
                "type": "user",
                "role": "owner",
                "emailAddress": "owner@example.com",
                "permissionDetails": [{ "inherited": false, "permissionType": "file", "role": "owner" }]
            },
            {
                "id": "perm-domain",
                "type": "domain",
                "role": "reader",
                "domain": "example.com",
                "allowFileDiscovery": false,
                "permissionDetails": [{ "inherited": true, "permissionType": "file", "role": "reader" }]
            }
        ],
        "webViewLink": "https://example.com/file",
        "quotaBytesUsed": "1024",
        "imageMediaMetadata": {
            "width": 4032,
            "height": 3024,
            "cameraMake": "Google",
            "cameraModel": "Pixel",
            "time": "2024:01:02 03:04:05",
            "exposureTime": 0.005,
            "aperture": 1.8,
            "focalLength": 5.43,
            "isoSpeed": 200
        }
    }))
    .expect("google drive file");

    let mapped_file = map_drive_file(file).expect("mapped file");
    assert_eq!(mapped_file.id, "photo-1");
    assert!(mapped_file.operator_can_share_manage);
    assert_eq!(mapped_file.permissions.len(), 4);
    assert!(mapped_file
        .permissions
        .iter()
        .any(|permission| { permission.permission_type == "group" && permission.actionable }));
    assert!(mapped_file
        .permissions
        .iter()
        .any(|permission| { permission.role == "owner" && !permission.actionable }));
    assert!(mapped_file.permissions.iter().any(|permission| {
        permission.permission_type == "domain" && permission.inherited && !permission.actionable
    }));
    assert_eq!(
        mapped_file.image_media_metadata.as_ref().and_then(|metadata| metadata.iso_speed),
        Some(200)
    );

    let change_page: google_drive3::api::ChangeList = serde_json::from_value(json!({
        "nextPageToken": "delta-2",
        "newStartPageToken": "delta-3",
        "changes": [
            { "fileId": "deleted-1", "removed": true },
            {
                "fileId": "photo-1",
                "removed": false,
                "file": {
                    "id": "photo-1",
                    "name": "Photo.jpg",
                    "mimeType": "image/jpeg",
                    "ownedByMe": true,
                    "capabilities": { "canShare": true }
                }
            }
        ]
    }))
    .expect("change list");
    let mapped_changes = map_change_list(change_page).expect("mapped changes");
    assert_eq!(mapped_changes.removed_file_ids, vec!["deleted-1"]);
    assert_eq!(mapped_changes.updated_files.len(), 1);
    assert_eq!(mapped_changes.next_page_token.as_deref(), Some("delta-2"));
    assert_eq!(mapped_changes.new_start_page_token.as_deref(), Some("delta-3"));

    let temp_dir = TempDir::new().expect("tempdir");
    let token_path = temp_dir.path().join("tokens.json");
    std::fs::write(
        &token_path,
        serde_json::to_string_pretty(&json!([
            {
                "scopes": ["scope-a"],
                "token": { "access_token": "access-1", "refresh_token": "refresh-1" }
            },
            {
                "scopes": ["scope-b"],
                "token": { "access_token": "access-1", "refresh_token": "refresh-2" }
            }
        ]))
        .expect("token json"),
    )
    .expect("write token file");
    assert_eq!(
        extract_revocable_tokens(&token_path).expect("tokens"),
        vec!["access-1".to_string(), "refresh-1".to_string(), "refresh-2".to_string()]
    );
}

#[test]
fn helper_functions_cover_mapping_and_google_error_branches() {
    assert_eq!(
        oauth_scope_urls(DriveScope::Drive),
        vec![
            "https://www.googleapis.com/auth/drive.metadata.readonly",
            "https://www.googleapis.com/auth/drive.readonly",
            "https://www.googleapis.com/auth/drive"
        ]
    );
    assert_eq!(highest_active_scope(&[]), DriveScope::MetadataReadonly);
    assert_eq!(
        highest_active_scope(&[DriveScope::MetadataReadonly, DriveScope::Drive]),
        DriveScope::Drive
    );
    assert_eq!(format_float(2.0), "2");
    assert!(parse_drive_image_time("2024-01-02T03:04:05Z").is_some());

    let no_user = account_from_about(google_drive3::api::About::default()).expect_err("no user");
    assert!(no_user.to_string().contains("authenticated user"));
    let no_permission_id = account_from_about(google_drive3::api::About {
        user: Some(google_drive3::api::User {
            email_address: Some("user@example.com".into()),
            ..google_drive3::api::User::default()
        }),
        ..google_drive3::api::About::default()
    })
    .expect_err("no permission id");
    assert!(no_permission_id.to_string().contains("permissionId"));
    let no_email = account_from_about(google_drive3::api::About {
        user: Some(google_drive3::api::User {
            permission_id: Some("perm".into()),
            ..google_drive3::api::User::default()
        }),
        ..google_drive3::api::About::default()
    })
    .expect_err("no email");
    assert!(no_email.to_string().contains("email address"));

    let no_file_change = google_drive3::api::Change {
        file_id: Some("file-1".into()),
        removed: Some(false),
        file: None,
        ..google_drive3::api::Change::default()
    };
    let mut removed = Vec::new();
    let mut updated = Vec::new();
    apply_change(no_file_change, &mut removed, &mut updated).expect("apply change");
    assert!(removed.is_empty());
    assert!(updated.is_empty());

    let missing_id = map_drive_file(google_drive3::api::File {
        name: Some("Missing".into()),
        mime_type: Some("text/plain".into()),
        ..google_drive3::api::File::default()
    })
    .expect_err("missing id");
    assert!(missing_id.to_string().contains("file.id"));

    let owner_fallback = map_drive_file(google_drive3::api::File {
        id: Some("owned".into()),
        name: Some("Owned".into()),
        mime_type: Some("text/plain".into()),
        owned_by_me: Some(true),
        ..google_drive3::api::File::default()
    })
    .expect("owned fallback");
    assert!(owner_fallback.operator_can_share_manage);

    let pending_group = map_drive_permission(google_drive3::api::Permission {
        id: Some("perm-group".into()),
        type_: Some("group".into()),
        role: Some("reader".into()),
        pending_owner: Some(true),
        permission_details: Some(Vec::new()),
        ..google_drive3::api::Permission::default()
    });
    assert!(!pending_group.actionable);

    let oauth_error = yup_oauth2::Error::AuthError(yup_oauth2::error::AuthError {
        error: yup_oauth2::error::AuthErrorCode::InvalidGrant,
        error_description: None,
        error_uri: None,
    });
    assert!(render_oauth_error(&oauth_error).contains("revoked or expired"));
    let other_oauth_error = yup_oauth2::Error::MissingAccessToken;
    assert!(render_oauth_error(&other_oauth_error).contains("Google OAuth failed"));
    let mapped_oauth_error = map_token_error(Box::new(oauth_error));
    assert!(mapped_oauth_error.to_string().contains("revoked or expired"));
    let mapped_string_error = map_token_error(Box::new(std::io::Error::other("invalid_grant")));
    assert!(mapped_string_error.to_string().contains("invalid_grant"));
    let mapped_generic_error = map_token_error(Box::new(std::io::Error::other("boom")));
    assert!(mapped_generic_error.to_string().contains("failed to obtain"));

    let bad_request = map_google_error(
        "listing changes",
        common::Error::BadRequest(json!({ "error": { "message": "Page token is invalid" } })),
    );
    assert!(bad_request.to_string().contains("invalid page token"));
    let direct_message = map_google_error(
        "listing changes",
        common::Error::BadRequest(json!({ "message": "plain message" })),
    );
    assert_eq!(direct_message.to_string(), "plain message");
    let opaque_bad_request = map_google_error(
        "listing changes",
        common::Error::BadRequest(json!({ "detail": "unknown" })),
    );
    assert!(opaque_bad_request.to_string().contains("Google Drive rejected the request"));
    let gone_response = google_drive3::hyper::Response::builder()
        .status(410)
        .body(common::to_body::<String>(None))
        .unwrap();
    assert!(map_google_error("syncing", common::Error::Failure(gone_response))
        .to_string()
        .contains("410 Gone"));
    let other_response = google_drive3::hyper::Response::builder()
        .status(500)
        .body(common::to_body::<String>(None))
        .unwrap();
    assert!(map_google_error("syncing", common::Error::Failure(other_response))
        .to_string()
        .contains("HTTP status 500"));
    let missing_token = map_google_error(
        "syncing",
        common::Error::MissingToken(Box::new(std::io::Error::other("no token"))),
    );
    assert!(missing_token.to_string().contains("failed to obtain"));
    let other_google_error = map_google_error("syncing", common::Error::Cancelled);
    assert!(other_google_error.to_string().contains("Operation cancelled"));

    assert_eq!(
        extract_google_error_message(&json!({ "error": { "message": "nested" } })),
        Some("nested".into())
    );
    assert_eq!(
        extract_google_error_message(&json!({ "message": "top-level" })),
        Some("top-level".into())
    );
}

#[test]
fn live_query_helpers_escape_and_filter_release_files() {
    assert_eq!(escape_drive_query("root\\child's"), "root\\\\child\\'s");
    assert_eq!(
        drive_find_file_query("folder'id", "inventory's.db"),
        "'folder\\'id' in parents and name = 'inventory\\'s.db' and trashed = false"
    );
    assert_eq!(
        drive_folder_listing_query("folder\\id", Some("inventory.")),
        "'folder\\\\id' in parents and trashed = false and name contains 'inventory.'"
    );
    assert_eq!(
        drive_folder_listing_query("folder-id", None),
        "'folder-id' in parents and trashed = false"
    );

    let files = vec![
        RemoteFileMetadata {
            id: "2".into(),
            name: "inventory.z.db".into(),
            mime_type: "application/vnd.sqlite3".into(),
            size: Some(2),
            modified_time: None,
            owned_by_me: true,
            shared: false,
            permissions: Vec::new(),
        },
        RemoteFileMetadata {
            id: "1".into(),
            name: "inventory.a.db".into(),
            mime_type: "application/vnd.sqlite3".into(),
            size: Some(1),
            modified_time: None,
            owned_by_me: true,
            shared: false,
            permissions: Vec::new(),
        },
        RemoteFileMetadata {
            id: "3".into(),
            name: "other.db".into(),
            mime_type: "application/vnd.sqlite3".into(),
            size: Some(3),
            modified_time: None,
            owned_by_me: true,
            shared: false,
            permissions: Vec::new(),
        },
    ];
    let filtered = filter_remote_files_by_prefix(files, Some("inventory."));
    assert_eq!(
        filtered.iter().map(|file| file.name.as_str()).collect::<Vec<_>>(),
        vec!["inventory.a.db", "inventory.z.db"]
    );
}

#[test]
fn live_request_helpers_build_expected_payloads_and_exif_results() {
    let folder = create_folder_request("root", "Snapshots");
    assert_eq!(folder.name.as_deref(), Some("Snapshots"));
    assert_eq!(folder.mime_type.as_deref(), Some(gdrive_core::GOOGLE_DRIVE_FOLDER_MIME));
    assert_eq!(folder.parents.as_deref(), Some(&["root".to_string()][..]));

    let copy = copy_file_request("folder-1", Some("Copy.txt"));
    assert_eq!(copy.name.as_deref(), Some("Copy.txt"));
    assert_eq!(copy.parents.as_deref(), Some(&["folder-1".to_string()][..]));
    let unnamed_copy = copy_file_request("folder-1", None);
    assert!(unnamed_copy.name.is_none());

    let upload = upload_file_request("folder-1", "inventory.db", "application/vnd.sqlite3");
    assert_eq!(upload.name.as_deref(), Some("inventory.db"));
    assert_eq!(upload.mime_type.as_deref(), Some("application/vnd.sqlite3"));
    assert_eq!(upload.parents.as_deref(), Some(&["folder-1".to_string()][..]));

    let update = update_file_request("inventory.db", "application/vnd.sqlite3");
    assert_eq!(update.name.as_deref(), Some("inventory.db"));
    assert_eq!(update.mime_type.as_deref(), Some("application/vnd.sqlite3"));
    assert!(update.parents.is_none());

    let trash = trash_file_request();
    assert_eq!(trash.trashed, Some(true));

    let exif = inspect_exif_details_from_record(
        "photo",
        FileRecord {
            id: "photo".into(),
            name: "Photo.jpg".into(),
            mime_type: "image/jpeg".into(),
            web_view_link: Some("https://drive.example/photo".into()),
            image_media_metadata: Some(ImageMediaMetadata {
                width: Some(10),
                height: Some(20),
                ..ImageMediaMetadata::default()
            }),
            ..FileRecord::default()
        },
    )
    .expect("exif details");
    assert_eq!(exif.file_id, "photo");
    assert_eq!(exif.source, ExifSource::DriveImageMediaMetadata);

    let non_image = inspect_exif_details_from_record(
        "text",
        FileRecord {
            id: "text".into(),
            name: "Notes.txt".into(),
            mime_type: "text/plain".into(),
            ..FileRecord::default()
        },
    )
    .expect_err("non image");
    assert!(non_image.to_string().contains("not an image"));

    let missing_metadata = inspect_exif_details_from_record(
        "photo",
        FileRecord {
            id: "photo".into(),
            name: "Photo.jpg".into(),
            mime_type: "image/jpeg".into(),
            ..FileRecord::default()
        },
    )
    .expect_err("missing metadata");
    assert!(missing_metadata.to_string().contains("no imageMediaMetadata"));
}

#[test]
fn live_retry_helpers_classify_transient_google_errors() {
    let rate_limited = google_drive3::hyper::Response::builder()
        .status(429)
        .body(common::to_body::<String>(None))
        .unwrap();
    assert!(is_retryable_google_error(&common::Error::Failure(rate_limited)));

    let server_error = google_drive3::hyper::Response::builder()
        .status(503)
        .body(common::to_body::<String>(None))
        .unwrap();
    assert!(is_retryable_google_error(&common::Error::Failure(server_error)));

    let not_found = google_drive3::hyper::Response::builder()
        .status(404)
        .body(common::to_body::<String>(None))
        .unwrap();
    assert!(!is_retryable_google_error(&common::Error::Failure(not_found)));

    assert!(is_retryable_google_error(&common::Error::BadRequest(json!({
        "error": { "message": "User rate limit exceeded" }
    }))));
    assert!(!is_retryable_google_error(&common::Error::BadRequest(json!({
        "error": { "message": "invalid page token" }
    }))));

    assert_eq!(retry_delay(1), std::time::Duration::from_millis(500));
    assert_eq!(retry_delay(3), std::time::Duration::from_millis(2000));
}

#[test]
fn mock_gateway_covers_scope_upgrade_mutation_and_file_helpers() {
    let temp_dir = TempDir::new().expect("tempdir");
    let fixture_dir = write_fixture(&temp_dir, sample_fixture());
    let gateway = MockDriveGateway::new(&fixture_dir, temp_dir.path().join("mock-auth.json"));
    let runtime = tokio::runtime::Runtime::new().expect("runtime");

    let session = runtime.block_on(gateway.login(DriveScope::MetadataReadonly)).expect("login");
    assert_eq!(session.account.email, "mock@example.com");
    assert!(runtime.block_on(gateway.auth_status()).expect("status").session.is_some());
    assert_eq!(
        runtime.block_on(gateway.get_start_page_token()).expect("start page token"),
        "start-token-1"
    );
    assert_eq!(runtime.block_on(gateway.list_files(None)).expect("files").files.len(), 1);
    assert_eq!(runtime.block_on(gateway.get_file("photo-file")).expect("file").id, "photo-file");
    assert_eq!(
        runtime.block_on(gateway.inspect_exif("photo-file")).expect("inspect exif").source,
        ExifSource::DriveImageMediaMetadata
    );
    runtime.block_on(gateway.ensure_scope(DriveScope::Drive)).expect("ensure drive scope");
    runtime.block_on(gateway.delete_permission("photo-file", "perm-1")).expect("delete permission");
    let file = runtime.block_on(gateway.get_file("photo-file")).expect("file after delete");
    assert!(file.permissions.is_empty());
    assert!(!file.shared);
    assert!(runtime.block_on(gateway.delete_permission("photo-file", "missing")).is_err());
    assert!(runtime.block_on(gateway.inspect_exif("missing")).is_err());
    assert!(runtime
        .block_on(gateway.inspect_exif("photo-file"))
        .expect("exif")
        .metadata
        .width
        .is_some());

    let folder = runtime.block_on(gateway.create_folder("root", "Remote DB")).expect("folder");
    let uploaded = runtime
        .block_on(gateway.upload_file_to_folder(
            &folder.id,
            "inventory.db",
            "application/vnd.sqlite3",
            b"db-v1".to_vec(),
        ))
        .expect("upload remote db");
    assert_eq!(uploaded.name, "inventory.db");
    assert_eq!(
        runtime
            .block_on(gateway.find_file_in_folder(&folder.id, "inventory.db"))
            .expect("find remote")
            .expect("remote file")
            .id,
        uploaded.id
    );
    assert_eq!(runtime.block_on(gateway.download_file(&uploaded.id)).expect("download"), b"db-v1");
    runtime
        .block_on(gateway.update_file_contents(
            &uploaded.id,
            "inventory.db",
            "application/vnd.sqlite3",
            b"db-v2".to_vec(),
        ))
        .expect("update remote db");
    assert_eq!(
        runtime.block_on(gateway.download_file(&uploaded.id)).expect("download updated"),
        b"db-v2"
    );

    runtime.block_on(gateway.trash_file("photo-file")).expect("trash file");
    assert!(runtime.block_on(gateway.get_file("photo-file")).is_err());
    let files_after_trash =
        runtime.block_on(gateway.list_files(None)).expect("files after trash").files;
    assert!(!files_after_trash.iter().any(|file| file.id == "photo-file"));
}

#[test]
fn mock_gateway_failure_modes_and_session_files_are_covered() {
    let temp_dir = TempDir::new().expect("tempdir");
    let fixture_dir = write_fixture(
        &temp_dir,
        r#"{
  "account": { "account_id": "account-1", "email": "mock@example.com", "display_name": "Mock" },
  "start_page_token": "start-token-1",
  "file_pages": {},
  "change_pages": {},
  "failure_modes": {
    "auth_status_error": "revoked",
    "list_changes_errors": { "bad-token": "invalid token" }
  }
}"#,
    );
    let state_path = temp_dir.path().join("mock-auth.json");
    let gateway = MockDriveGateway::new(&fixture_dir, &state_path);
    let runtime = tokio::runtime::Runtime::new().expect("runtime");

    write_session_file(
        &state_path,
        &AuthSession {
            account: AccountProfile {
                account_id: "account-1".into(),
                email: "mock@example.com".into(),
                display_name: None,
            },
            active_scopes: vec![DriveScope::MetadataReadonly],
        },
    )
    .expect("write session");
    assert!(read_session_file(&state_path).expect("read").is_some());
    assert!(runtime.block_on(gateway.auth_status()).is_err());
    assert!(runtime.block_on(gateway.list_changes("bad-token")).is_err());
    assert!(delete_file_if_exists(&state_path).expect("delete"));
    assert!(!delete_file_if_exists(&state_path).expect("delete missing"));

    let mut file = FileRecord {
        id: "file-1".into(),
        shared: true,
        permissions: vec![PermissionRecord { id: "perm-1".into(), ..PermissionRecord::default() }],
        ..FileRecord::default()
    };
    apply_deleted_permissions(
        &mut file,
        &[DeletedPermission { file_id: "file-1".into(), permission_id: "perm-1".into() }],
    );
    assert!(!file.shared);
}

#[test]
fn mock_gateway_missing_pages_and_exif_errors_are_covered() {
    let temp_dir = TempDir::new().expect("tempdir");
    let fixture_dir = write_fixture(
        &temp_dir,
        r#"{
  "account": { "account_id": "account-1", "email": "mock@example.com", "display_name": "Mock" },
  "start_page_token": "start-token-1",
  "file_pages": {
    "__start__": {
      "next_page_token": null,
      "files": [
        { "id": "text-file", "name": "Notes.txt", "mime_type": "text/plain" },
        { "id": "image-no-meta", "name": "Photo.jpg", "mime_type": "image/jpeg" }
      ]
    }
  },
  "change_pages": {
    "start-token-1": {
      "next_page_token": null,
      "new_start_page_token": "start-token-2",
      "removed_file_ids": [],
      "updated_files": []
    }
  }
}"#,
    );
    let gateway = MockDriveGateway::new(&fixture_dir, temp_dir.path().join("mock-auth.json"));
    let runtime = tokio::runtime::Runtime::new().expect("runtime");

    let ensure_scope_error =
        runtime.block_on(gateway.ensure_scope(DriveScope::MetadataReadonly)).expect_err("scope");
    assert!(ensure_scope_error.to_string().contains("not logged in"));

    runtime.block_on(gateway.login(DriveScope::MetadataReadonly)).expect("login");
    assert!(runtime.block_on(gateway.list_files(Some("missing"))).is_err());
    assert!(runtime.block_on(gateway.list_changes("missing")).is_err());
    assert!(runtime.block_on(gateway.inspect_exif("text-file")).is_err());
    assert!(runtime.block_on(gateway.inspect_exif("image-no-meta")).is_err());
}

#[test]
fn cascaded_deletions_propagate_folder_grant_to_descendants() {
    let shared = |id: &str, parent: &str| FileRecord {
        id: id.into(),
        parents: vec![parent.into()],
        permissions: vec![PermissionRecord { id: "perm".into(), ..PermissionRecord::default() }],
        ..FileRecord::default()
    };
    let folder = shared("folder", "root");
    let child = shared("child", "folder");
    let grandchild = shared("grandchild", "child");
    let sibling = shared("sibling", "root");
    let all = vec![&folder, &child, &grandchild, &sibling];

    let deletions =
        vec![DeletedPermission { file_id: "folder".into(), permission_id: "perm".into() }];
    let expanded = expand_cascaded_deletions(&all, &deletions);
    let ids: BTreeSet<&str> = expanded.iter().map(|deletion| deletion.file_id.as_str()).collect();

    // The grant is removed from the folder and every descendant that carries it,
    // but a sibling outside the folder keeps its own (separate) share.
    assert_eq!(
        ids,
        BTreeSet::from(["folder", "child", "grandchild"]),
        "cascade must reach descendants only"
    );
}
