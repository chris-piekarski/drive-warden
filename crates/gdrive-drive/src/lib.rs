use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use chrono::{DateTime, NaiveDateTime, Utc};
use gdrive_core::{
    AccountProfile, AuthSession, AuthStatus, ChangeListPage, CoreError, CoreResult, DriveGateway,
    DriveScope, ExifSource, FileListPage, FileRecord, ImageMediaMetadata, InspectExifDetails,
    PermissionRecord, RemoteFileMetadata, GOOGLE_DRIVE_FOLDER_MIME,
};
use google_drive3::api::{About, Change, ChangeList as GoogleChangeList, File as GoogleFile};
use google_drive3::common;
use serde::{Deserialize, Serialize};
use serde_json::Value;

const FILE_PAGE_SIZE: i32 = 1000;
const FILE_ITEM_FIELDS: &str = "id,name,mimeType,parents,trashed,ownedByMe,shared,capabilities(canShare),size,md5Checksum,modifiedTime,viewedByMeTime,permissions(id,type,role,emailAddress,domain,allowFileDiscovery,displayName,deleted,pendingOwner,permissionDetails(inherited,inheritedFrom,permissionType,role)),webViewLink,quotaBytesUsed,imageMediaMetadata(width,height,cameraMake,cameraModel,time,exposureTime,aperture,focalLength,isoSpeed)";
const FILE_FIELDS: &str = "nextPageToken,files(id,name,mimeType,parents,trashed,ownedByMe,shared,capabilities(canShare),size,md5Checksum,modifiedTime,viewedByMeTime,permissions(id,type,role,emailAddress,domain,allowFileDiscovery,displayName,deleted,pendingOwner,permissionDetails(inherited,inheritedFrom,permissionType,role)),webViewLink,quotaBytesUsed,imageMediaMetadata(width,height,cameraMake,cameraModel,time,exposureTime,aperture,focalLength,isoSpeed))";
const CHANGE_FIELDS: &str = "nextPageToken,newStartPageToken,changes(fileId,removed,file(id,name,mimeType,parents,trashed,ownedByMe,shared,capabilities(canShare),size,md5Checksum,modifiedTime,viewedByMeTime,permissions(id,type,role,emailAddress,domain,allowFileDiscovery,displayName,deleted,pendingOwner,permissionDetails(inherited,inheritedFrom,permissionType,role)),webViewLink,quotaBytesUsed,imageMediaMetadata(width,height,cameraMake,cameraModel,time,exposureTime,aperture,focalLength,isoSpeed)))";
const ABOUT_FIELDS: &str = "user(permissionId,emailAddress,displayName)";

#[derive(Debug, Clone)]
pub struct GoogleDriveGateway {
    credentials_path: PathBuf,
    token_path: PathBuf,
    session_path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct MockDriveGateway {
    fixture_dir: PathBuf,
    state_path: PathBuf,
    mutation_state_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredSession {
    session: AuthSession,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GoogleSessionState {
    version: u32,
    account: AccountProfile,
    active_scopes: Vec<DriveScope>,
    credentials_path: String,
    token_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MockFixture {
    account: AccountProfile,
    start_page_token: String,
    file_pages: BTreeMap<String, FileListPage>,
    change_pages: BTreeMap<String, ChangeListPage>,
    #[serde(default)]
    failure_modes: MockFailureModes,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct MockFailureModes {
    auth_status_error: Option<String>,
    #[serde(default)]
    list_changes_errors: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct MockMutationState {
    deleted_permissions: Vec<DeletedPermission>,
    #[serde(default)]
    created_files: Vec<FileRecord>,
    #[serde(default)]
    created_counter: usize,
    #[serde(default)]
    trashed_file_ids: Vec<String>,
    #[serde(default)]
    parent_moves: Vec<ParentMove>,
    #[serde(default)]
    remote_files: Vec<MockRemoteFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ParentMove {
    file_id: String,
    add_parent_id: String,
    remove_parent_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct MockRemoteFile {
    metadata: FileRecord,
    contents: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct DeletedPermission {
    file_id: String,
    permission_id: String,
}

impl GoogleDriveGateway {
    pub fn new<P: AsRef<Path>>(session_path: P) -> Self {
        let session_path = session_path.as_ref().to_path_buf();
        let runtime_dir =
            session_path.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."));
        Self {
            credentials_path: runtime_dir.join("credentials.json"),
            token_path: runtime_dir.join("google-tokens.json"),
            session_path,
        }
    }

    pub fn with_paths<P: AsRef<Path>, Q: AsRef<Path>, R: AsRef<Path>>(
        credentials_path: P,
        token_path: Q,
        session_path: R,
    ) -> Self {
        Self {
            credentials_path: credentials_path.as_ref().to_path_buf(),
            token_path: token_path.as_ref().to_path_buf(),
            session_path: session_path.as_ref().to_path_buf(),
        }
    }

    fn read_session(&self) -> CoreResult<Option<AuthSession>> {
        Ok(self.load_google_session_state()?.map(|stored| AuthSession {
            account: stored.account,
            active_scopes: stored.active_scopes,
        }))
    }

    fn load_google_session_state(&self) -> CoreResult<Option<GoogleSessionState>> {
        match std::fs::read_to_string(&self.session_path) {
            Ok(contents) => {
                if let Ok(session) = serde_json::from_str::<GoogleSessionState>(&contents) {
                    if !self.token_path.exists() {
                        return Err(CoreError::Message(format!(
                            "stored Google session metadata exists but token cache `{}` is missing; run `drive-warden auth login` again",
                            self.token_path.display()
                        )));
                    }
                    return Ok(Some(session));
                }
                if serde_json::from_str::<StoredSession>(&contents).is_ok() {
                    return Err(CoreError::Message(
                        "legacy Google session format detected; run `drive-warden auth logout` then `drive-warden auth login` to refresh the live session".into(),
                    ));
                }
                Err(CoreError::Message(format!(
                    "failed to parse Google session metadata at `{}`",
                    self.session_path.display()
                )))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(CoreError::from(error)),
        }
    }

    fn write_google_session_state(&self, state: &GoogleSessionState) -> CoreResult<()> {
        ensure_parent_dir(&self.session_path)?;
        let contents = serde_json::to_string_pretty(state)?;
        std::fs::write(&self.session_path, contents)?;
        ensure_private_file_permissions(&self.session_path)?;
        Ok(())
    }

    fn highest_scope_from_session(
        session: Option<&GoogleSessionState>,
        requested: DriveScope,
    ) -> DriveScope {
        session
            .and_then(|stored| {
                stored.active_scopes.iter().copied().max_by_key(|scope| scope_rank(*scope))
            })
            .map(|existing| max_scope(existing, requested))
            .unwrap_or(requested)
    }

    async fn ensure_scope_internal(&self, scope: DriveScope) -> CoreResult<AuthSession> {
        let Some(session) = self.load_google_session_state()? else {
            return Err(CoreError::Message(
                "not logged in; run `drive-warden auth login` first".into(),
            ));
        };
        let target_scope = max_scope(
            session
                .active_scopes
                .iter()
                .copied()
                .max_by_key(|item| scope_rank(*item))
                .unwrap_or(DriveScope::MetadataReadonly),
            scope,
        );
        if session.active_scopes.contains(&scope) {
            return Ok(AuthSession {
                account: session.account,
                active_scopes: session.active_scopes,
            });
        }
        self.perform_login(target_scope, Some(&session)).await
    }
}

impl MockDriveGateway {
    pub fn new<P: AsRef<Path>, Q: AsRef<Path>>(fixture_dir: P, state_path: Q) -> Self {
        let state_path = state_path.as_ref().to_path_buf();
        Self {
            fixture_dir: fixture_dir.as_ref().to_path_buf(),
            mutation_state_path: state_path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join("mock-drive-state.json"),
            state_path,
        }
    }

    fn fixture(&self) -> CoreResult<MockFixture> {
        let fixture_path = self.fixture_dir.join("api/mock-drive.json");
        let contents = std::fs::read_to_string(&fixture_path)?;
        let fixture = serde_json::from_str(&contents).map_err(CoreError::from)?;
        self.apply_mutations(fixture)
    }

    fn read_session(&self) -> CoreResult<Option<AuthSession>> {
        read_session_file(&self.state_path)
    }

    fn write_session(&self, session: &AuthSession) -> CoreResult<()> {
        write_session_file(&self.state_path, session)
    }

    fn clear_session(&self) -> CoreResult<bool> {
        delete_file_if_exists(&self.state_path)
    }

    fn load_mutation_state(&self) -> CoreResult<MockMutationState> {
        match std::fs::read_to_string(&self.mutation_state_path) {
            Ok(contents) => serde_json::from_str(&contents).map_err(CoreError::from),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(MockMutationState::default())
            }
            Err(error) => Err(CoreError::from(error)),
        }
    }

    fn store_mutation_state(&self, state: &MockMutationState) -> CoreResult<()> {
        if let Some(parent_dir) = self.mutation_state_path.parent() {
            std::fs::create_dir_all(parent_dir)?;
        }
        let contents = serde_json::to_string_pretty(state)?;
        std::fs::write(&self.mutation_state_path, contents)?;
        Ok(())
    }

    fn apply_mutations(&self, mut fixture: MockFixture) -> CoreResult<MockFixture> {
        let state = self.load_mutation_state()?;
        // Deleting a grant on a folder cascades to inherited descendants in Google
        // Drive; expand the recorded deletions to model that against the fixture.
        let deletions = {
            let all_files: Vec<&FileRecord> = fixture
                .file_pages
                .values()
                .flat_map(|page| page.files.iter())
                .chain(state.created_files.iter())
                .collect();
            expand_cascaded_deletions(&all_files, &state.deleted_permissions)
        };
        for page in fixture.file_pages.values_mut() {
            for file in &mut page.files {
                apply_deleted_permissions(file, &deletions);
                apply_parent_moves(file, &state.parent_moves);
            }
            page.files.retain(|file| !state.trashed_file_ids.contains(&file.id));
        }
        for page in fixture.change_pages.values_mut() {
            for file in &mut page.updated_files {
                apply_deleted_permissions(file, &deletions);
                apply_parent_moves(file, &state.parent_moves);
            }
            page.updated_files.retain(|file| !state.trashed_file_ids.contains(&file.id));
        }
        if let Some(first_page) = fixture.file_pages.get_mut("__start__") {
            first_page.files.extend(
                state
                    .created_files
                    .iter()
                    .filter(|file| !state.trashed_file_ids.contains(&file.id))
                    .cloned()
                    .map(|mut file| {
                        apply_parent_moves(&mut file, &state.parent_moves);
                        file
                    }),
            );
            first_page.files.extend(
                state
                    .remote_files
                    .iter()
                    .map(|file| file.metadata.clone())
                    .filter(|file| !state.trashed_file_ids.contains(&file.id))
                    .map(|mut file| {
                        apply_parent_moves(&mut file, &state.parent_moves);
                        file
                    }),
            );
        }
        Ok(fixture)
    }

    fn require_active_session(&self) -> CoreResult<AuthSession> {
        self.read_session()?.ok_or_else(|| {
            CoreError::Message("not logged in; run `drive-warden auth login` first".into())
        })
    }

    fn validate_session(&self, fixture: &MockFixture) -> CoreResult<()> {
        if let Some(message) = &fixture.failure_modes.auth_status_error {
            return Err(CoreError::Message(message.clone()));
        }
        Ok(())
    }

    fn next_created_id(state: &mut MockMutationState, prefix: &str) -> String {
        state.created_counter += 1;
        format!("mock-{prefix}-{}", state.created_counter)
    }

    fn private_owner_permission(&self) -> CoreResult<PermissionRecord> {
        let session = self.require_active_session()?;
        Ok(PermissionRecord {
            id: format!("owner-{}", session.account.account_id),
            permission_type: "user".into(),
            role: "owner".into(),
            email_address: Some(session.account.email),
            actionable: false,
            ..PermissionRecord::default()
        })
    }

    fn parent_exists_in_fixture_or_state(
        fixture: &MockFixture,
        state: &MockMutationState,
        parent_id: &str,
    ) -> bool {
        parent_id == "root"
            || fixture
                .file_pages
                .values()
                .flat_map(|page| page.files.iter())
                .chain(state.created_files.iter())
                .chain(state.remote_files.iter().map(|file| &file.metadata))
                .any(|file| file.id == parent_id && file.mime_type == GOOGLE_DRIVE_FOLDER_MIME)
    }
}

#[async_trait]
impl DriveGateway for MockDriveGateway {
    async fn login(&self, scope: DriveScope) -> CoreResult<AuthSession> {
        let fixture = self.fixture()?;
        let mut session = self
            .read_session()?
            .unwrap_or(AuthSession { account: fixture.account, active_scopes: Vec::new() });

        if !session.active_scopes.contains(&scope) {
            session.active_scopes.push(scope);
        }

        self.write_session(&session)?;
        Ok(session)
    }

    async fn logout(&self) -> CoreResult<bool> {
        self.clear_session()
    }

    async fn auth_status(&self) -> CoreResult<AuthStatus> {
        let session = self.read_session()?;
        if session.is_some() {
            self.validate_session(&self.fixture()?)?;
        }
        Ok(AuthStatus { session })
    }

    async fn list_files(&self, page_token: Option<&str>) -> CoreResult<FileListPage> {
        let fixture = self.fixture()?;
        self.require_active_session()?;
        self.validate_session(&fixture)?;
        let key = page_token.unwrap_or("__start__");
        fixture.file_pages.get(key).cloned().ok_or_else(|| {
            CoreError::Message(format!("mock files page `{key}` was not found in fixture"))
        })
    }

    async fn get_start_page_token(&self) -> CoreResult<String> {
        let fixture = self.fixture()?;
        self.require_active_session()?;
        self.validate_session(&fixture)?;
        Ok(fixture.start_page_token)
    }

    async fn list_changes(&self, page_token: &str) -> CoreResult<ChangeListPage> {
        let fixture = self.fixture()?;
        self.require_active_session()?;
        self.validate_session(&fixture)?;
        if let Some(message) = fixture.failure_modes.list_changes_errors.get(page_token) {
            return Err(CoreError::Message(message.clone()));
        }
        fixture.change_pages.get(page_token).cloned().ok_or_else(|| {
            CoreError::Message(format!("mock changes page `{page_token}` was not found in fixture"))
        })
    }

    async fn get_file(&self, id: &str) -> CoreResult<FileRecord> {
        let fixture = self.fixture()?;
        self.require_active_session()?;
        self.validate_session(&fixture)?;
        fixture
            .file_pages
            .values()
            .flat_map(|page| page.files.iter())
            .chain(fixture.change_pages.values().flat_map(|page| page.updated_files.iter()))
            .find(|file| file.id == id)
            .cloned()
            .ok_or_else(|| CoreError::Message(format!("mock file `{id}` not found")))
    }

    async fn inspect_exif(&self, id: &str) -> CoreResult<InspectExifDetails> {
        let file = self.get_file(id).await?;
        if !file.mime_type.starts_with("image/") {
            return Err(CoreError::Message(format!(
                "file `{id}` is not an image; inspect exif only supports image/* items"
            )));
        }
        let Some(metadata) = file.image_media_metadata.clone() else {
            return Err(CoreError::Message(format!(
                "file `{id}` has no EXIF-compatible metadata in the current backend"
            )));
        };

        Ok(InspectExifDetails {
            file_id: file.id,
            name: file.name,
            mime_type: file.mime_type,
            web_view_link: file.web_view_link,
            source: ExifSource::DriveImageMediaMetadata,
            metadata,
        })
    }

    async fn ensure_scope(&self, scope: DriveScope) -> CoreResult<()> {
        let Some(mut session) = self.read_session()? else {
            return Err(CoreError::Message(
                "not logged in; run `drive-warden auth login` first".into(),
            ));
        };

        if !session.active_scopes.contains(&scope) {
            session.active_scopes.push(scope);
            self.write_session(&session)?;
        }

        self.validate_session(&self.fixture()?)?;

        Ok(())
    }

    async fn create_folder(&self, parent_id: &str, name: &str) -> CoreResult<FileRecord> {
        self.ensure_scope(DriveScope::Drive).await?;
        if parent_id != "root" {
            let fixture = self.fixture()?;
            let parent_exists = fixture
                .file_pages
                .values()
                .flat_map(|page| page.files.iter())
                .any(|file| file.id == parent_id && file.mime_type == GOOGLE_DRIVE_FOLDER_MIME);
            if !parent_exists {
                return Err(CoreError::Message(format!(
                    "mock parent folder `{parent_id}` was not found"
                )));
            }
        }

        let mut state = self.load_mutation_state()?;
        let created = FileRecord {
            id: Self::next_created_id(&mut state, "folder"),
            name: name.to_string(),
            mime_type: GOOGLE_DRIVE_FOLDER_MIME.into(),
            parents: vec![parent_id.to_string()],
            owned_by_me: true,
            shared: false,
            operator_can_share_manage: true,
            ..FileRecord::default()
        };
        state.created_files.push(created.clone());
        self.store_mutation_state(&state)?;
        Ok(created)
    }

    async fn copy_file(
        &self,
        file_id: &str,
        parent_id: &str,
        name: Option<&str>,
    ) -> CoreResult<FileRecord> {
        self.ensure_scope(DriveScope::Drive).await?;
        let source = self.get_file(file_id).await?;
        if source.mime_type == GOOGLE_DRIVE_FOLDER_MIME {
            return Err(CoreError::Message(format!(
                "mock file `{file_id}` is a folder; use create_folder for folder copies"
            )));
        }
        if parent_id != "root" {
            let fixture = self.fixture()?;
            let parent_exists = fixture
                .file_pages
                .values()
                .flat_map(|page| page.files.iter())
                .any(|file| file.id == parent_id && file.mime_type == GOOGLE_DRIVE_FOLDER_MIME);
            if !parent_exists {
                return Err(CoreError::Message(format!(
                    "mock parent folder `{parent_id}` was not found"
                )));
            }
        }

        let mut state = self.load_mutation_state()?;
        let copied = FileRecord {
            id: Self::next_created_id(&mut state, "copy"),
            name: name.unwrap_or(&source.name).to_string(),
            parents: vec![parent_id.to_string()],
            owned_by_me: true,
            shared: false,
            operator_can_share_manage: true,
            permissions: Vec::new(),
            web_view_link: None,
            ..source
        };
        state.created_files.push(copied.clone());
        self.store_mutation_state(&state)?;
        Ok(copied)
    }

    async fn delete_permission(&self, file_id: &str, permission_id: &str) -> CoreResult<()> {
        self.ensure_scope(DriveScope::Drive).await?;
        let fixture = self.fixture()?;
        let exists = fixture.file_pages.values().flat_map(|page| page.files.iter()).any(|file| {
            file.id == file_id
                && file.permissions.iter().any(|permission| permission.id == permission_id)
        });
        if !exists {
            return Err(CoreError::Message(format!(
                "mock permission `{permission_id}` on file `{file_id}` was not found"
            )));
        }

        let mut state = self.load_mutation_state()?;
        let deletion = DeletedPermission {
            file_id: file_id.to_string(),
            permission_id: permission_id.to_string(),
        };
        if !state.deleted_permissions.contains(&deletion) {
            state.deleted_permissions.push(deletion);
            self.store_mutation_state(&state)?;
        }
        Ok(())
    }

    async fn trash_file(&self, file_id: &str) -> CoreResult<()> {
        self.ensure_scope(DriveScope::Drive).await?;
        let fixture = self.fixture()?;
        let exists = fixture
            .file_pages
            .values()
            .flat_map(|page| page.files.iter())
            .any(|file| file.id == file_id)
            || fixture
                .change_pages
                .values()
                .flat_map(|page| page.updated_files.iter())
                .any(|file| file.id == file_id);
        if !exists {
            return Err(CoreError::Message(format!("mock file `{file_id}` was not found")));
        }

        let mut state = self.load_mutation_state()?;
        if !state.trashed_file_ids.iter().any(|trashed_id| trashed_id == file_id) {
            state.trashed_file_ids.push(file_id.to_string());
            self.store_mutation_state(&state)?;
        }
        Ok(())
    }

    async fn find_file_in_folder(
        &self,
        parent_id: &str,
        name: &str,
    ) -> CoreResult<Option<RemoteFileMetadata>> {
        let fixture = self.fixture()?;
        self.require_active_session()?;
        self.validate_session(&fixture)?;
        Ok(fixture
            .file_pages
            .values()
            .flat_map(|page| page.files.iter())
            .chain(fixture.change_pages.values().flat_map(|page| page.updated_files.iter()))
            .find(|file| file.name == name && file.parents.iter().any(|parent| parent == parent_id))
            .cloned()
            .map(RemoteFileMetadata::from))
    }

    async fn list_files_in_folder(
        &self,
        parent_id: &str,
        name_prefix: Option<&str>,
    ) -> CoreResult<Vec<RemoteFileMetadata>> {
        let fixture = self.fixture()?;
        self.require_active_session()?;
        self.validate_session(&fixture)?;
        let matches_parent_and_prefix = |file: &&FileRecord| {
            file.parents.iter().any(|parent| parent == parent_id)
                && name_prefix.is_none_or(|prefix| file.name.starts_with(prefix))
        };
        let mut files = fixture
            .file_pages
            .values()
            .flat_map(|page| page.files.iter())
            .chain(fixture.change_pages.values().flat_map(|page| page.updated_files.iter()))
            .filter(matches_parent_and_prefix)
            .cloned()
            .map(RemoteFileMetadata::from)
            .collect::<Vec<_>>();
        files
            .sort_by(|left, right| left.name.cmp(&right.name).then_with(|| left.id.cmp(&right.id)));
        Ok(files)
    }

    async fn upload_file_to_folder(
        &self,
        parent_id: &str,
        name: &str,
        mime_type: &str,
        contents: Vec<u8>,
    ) -> CoreResult<RemoteFileMetadata> {
        self.ensure_scope(DriveScope::Drive).await?;
        let fixture = self.fixture()?;
        let mut state = self.load_mutation_state()?;
        if !Self::parent_exists_in_fixture_or_state(&fixture, &state, parent_id) {
            return Err(CoreError::Message(format!(
                "mock parent folder `{parent_id}` was not found"
            )));
        }
        let metadata = FileRecord {
            id: Self::next_created_id(&mut state, "remote"),
            name: name.to_string(),
            mime_type: mime_type.to_string(),
            parents: vec![parent_id.to_string()],
            owned_by_me: true,
            shared: false,
            operator_can_share_manage: true,
            size: Some(contents.len() as u64),
            modified_time: Some(Utc::now()),
            permissions: vec![self.private_owner_permission()?],
            ..FileRecord::default()
        };
        state.remote_files.push(MockRemoteFile { metadata: metadata.clone(), contents });
        self.store_mutation_state(&state)?;
        Ok(RemoteFileMetadata::from(metadata))
    }

    async fn update_file_contents(
        &self,
        file_id: &str,
        name: &str,
        mime_type: &str,
        contents: Vec<u8>,
    ) -> CoreResult<RemoteFileMetadata> {
        self.ensure_scope(DriveScope::Drive).await?;
        let mut state = self.load_mutation_state()?;
        let Some(remote_file) =
            state.remote_files.iter_mut().find(|file| file.metadata.id == file_id)
        else {
            return Err(CoreError::Message(format!("mock remote file `{file_id}` was not found")));
        };
        remote_file.metadata.name = name.to_string();
        remote_file.metadata.mime_type = mime_type.to_string();
        remote_file.metadata.size = Some(contents.len() as u64);
        remote_file.metadata.modified_time = Some(Utc::now());
        remote_file.contents = contents;
        let metadata = remote_file.metadata.clone();
        self.store_mutation_state(&state)?;
        Ok(RemoteFileMetadata::from(metadata))
    }

    async fn rename_file(&self, file_id: &str, new_name: &str) -> CoreResult<RemoteFileMetadata> {
        self.ensure_scope(DriveScope::Drive).await?;
        let mut state = self.load_mutation_state()?;
        if let Some(file) = state.created_files.iter_mut().find(|file| file.id == file_id) {
            file.name = new_name.to_string();
            file.modified_time = Some(Utc::now());
            let metadata = file.clone();
            self.store_mutation_state(&state)?;
            return Ok(RemoteFileMetadata::from(metadata));
        }
        if let Some(remote_file) =
            state.remote_files.iter_mut().find(|file| file.metadata.id == file_id)
        {
            remote_file.metadata.name = new_name.to_string();
            remote_file.metadata.modified_time = Some(Utc::now());
            let metadata = remote_file.metadata.clone();
            self.store_mutation_state(&state)?;
            return Ok(RemoteFileMetadata::from(metadata));
        }
        Err(CoreError::Message(format!("mock remote file `{file_id}` was not found")))
    }

    async fn move_file(
        &self,
        file_id: &str,
        add_parent_id: &str,
        remove_parent_ids: &[String],
    ) -> CoreResult<RemoteFileMetadata> {
        self.ensure_scope(DriveScope::Drive).await?;
        let fixture = self.fixture()?;
        let mut state = self.load_mutation_state()?;
        if !Self::parent_exists_in_fixture_or_state(&fixture, &state, add_parent_id) {
            return Err(CoreError::Message(format!(
                "mock destination folder `{add_parent_id}` was not found"
            )));
        }
        let Some(mut file) = fixture
            .file_pages
            .values()
            .flat_map(|page| page.files.iter())
            .chain(fixture.change_pages.values().flat_map(|page| page.updated_files.iter()))
            .find(|file| file.id == file_id)
            .cloned()
        else {
            return Err(CoreError::Message(format!("mock file `{file_id}` was not found")));
        };
        if let Some(parent_id) = remove_parent_ids
            .iter()
            .find(|parent_id| !file.parents.iter().any(|parent| parent == *parent_id))
        {
            return Err(CoreError::Message(format!(
                "mock file `{file_id}` does not currently have parent `{parent_id}`"
            )));
        }

        let parent_move = ParentMove {
            file_id: file_id.to_string(),
            add_parent_id: add_parent_id.to_string(),
            remove_parent_ids: remove_parent_ids.to_vec(),
        };
        state.parent_moves.push(parent_move.clone());
        self.store_mutation_state(&state)?;
        apply_parent_moves(&mut file, &[parent_move]);
        Ok(RemoteFileMetadata::from(file))
    }

    async fn download_file(&self, file_id: &str) -> CoreResult<Vec<u8>> {
        self.ensure_scope(DriveScope::DriveReadonly).await?;
        let state = self.load_mutation_state()?;
        state
            .remote_files
            .iter()
            .find(|file| file.metadata.id == file_id)
            .map(|file| file.contents.clone())
            .ok_or_else(|| {
                CoreError::Message(format!("mock remote file `{file_id}` was not found"))
            })
    }
}

fn read_session_file(path: &Path) -> CoreResult<Option<AuthSession>> {
    match std::fs::read_to_string(path) {
        Ok(contents) => {
            let stored = serde_json::from_str::<StoredSession>(&contents)?;
            Ok(Some(stored.session))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(CoreError::from(error)),
    }
}

fn write_session_file(path: &Path, session: &AuthSession) -> CoreResult<()> {
    ensure_parent_dir(path)?;
    let contents = serde_json::to_string_pretty(&StoredSession { session: session.clone() })?;
    std::fs::write(path, contents)?;
    Ok(())
}

fn apply_deleted_permissions(file: &mut FileRecord, deletions: &[DeletedPermission]) {
    let before = file.permissions.len();
    file.permissions.retain(|permission| {
        !deletions
            .iter()
            .any(|deletion| deletion.file_id == file.id && deletion.permission_id == permission.id)
    });
    if before != file.permissions.len() {
        file.shared = !file.permissions.is_empty();
    }
}

fn apply_parent_moves(file: &mut FileRecord, moves: &[ParentMove]) {
    for parent_move in moves.iter().filter(|parent_move| parent_move.file_id == file.id) {
        file.parents.retain(|parent| !parent_move.remove_parent_ids.contains(parent));
        if !file.parents.iter().any(|parent| parent == &parent_move.add_parent_id) {
            file.parents.push(parent_move.add_parent_id.clone());
        }
        file.modified_time = Some(Utc::now());
    }
}

/// Models Google Drive's cascade: deleting a permission on a folder also removes
/// the inherited copies (same permission id) from every descendant. Returns the
/// recorded deletions plus the cascaded per-descendant deletions.
fn expand_cascaded_deletions(
    all_files: &[&FileRecord],
    deletions: &[DeletedPermission],
) -> Vec<DeletedPermission> {
    let mut children: HashMap<&str, Vec<&FileRecord>> = HashMap::new();
    for file in all_files {
        for parent in &file.parents {
            children.entry(parent.as_str()).or_default().push(file);
        }
    }

    let mut expanded: BTreeSet<(String, String)> = deletions
        .iter()
        .map(|deletion| (deletion.file_id.clone(), deletion.permission_id.clone()))
        .collect();

    for deletion in deletions {
        let mut stack = vec![deletion.file_id.as_str()];
        let mut visited: HashSet<&str> = HashSet::new();
        while let Some(current) = stack.pop() {
            if !visited.insert(current) {
                continue;
            }
            let Some(kids) = children.get(current) else {
                continue;
            };
            for kid in kids {
                if kid.permissions.iter().any(|permission| permission.id == deletion.permission_id)
                {
                    expanded.insert((kid.id.clone(), deletion.permission_id.clone()));
                }
                stack.push(kid.id.as_str());
            }
        }
    }

    expanded
        .into_iter()
        .map(|(file_id, permission_id)| DeletedPermission { file_id, permission_id })
        .collect()
}

fn ensure_parent_dir(path: &Path) -> CoreResult<()> {
    if let Some(parent_dir) = path.parent() {
        std::fs::create_dir_all(parent_dir)?;
    }
    Ok(())
}

fn ensure_private_file_permissions(path: &Path) -> CoreResult<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        if path.exists() {
            let permissions = std::fs::Permissions::from_mode(0o600);
            std::fs::set_permissions(path, permissions)?;
        }
    }
    Ok(())
}

fn delete_file_if_exists(path: &Path) -> CoreResult<bool> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(CoreError::from(error)),
    }
}

fn oauth_scope_urls(scope: DriveScope) -> Vec<&'static str> {
    cumulative_scopes(scope).into_iter().map(drive_scope_url).collect()
}

fn drive_scope_url(scope: DriveScope) -> &'static str {
    match scope {
        DriveScope::MetadataReadonly => "https://www.googleapis.com/auth/drive.metadata.readonly",
        DriveScope::DriveReadonly => "https://www.googleapis.com/auth/drive.readonly",
        DriveScope::Drive => "https://www.googleapis.com/auth/drive",
    }
}

fn cumulative_scopes(scope: DriveScope) -> Vec<DriveScope> {
    match scope {
        DriveScope::MetadataReadonly => vec![DriveScope::MetadataReadonly],
        DriveScope::DriveReadonly => {
            vec![DriveScope::MetadataReadonly, DriveScope::DriveReadonly]
        }
        DriveScope::Drive => {
            vec![DriveScope::MetadataReadonly, DriveScope::DriveReadonly, DriveScope::Drive]
        }
    }
}

fn highest_active_scope(scopes: &[DriveScope]) -> DriveScope {
    scopes
        .iter()
        .copied()
        .max_by_key(|scope| scope_rank(*scope))
        .unwrap_or(DriveScope::MetadataReadonly)
}

fn scope_rank(scope: DriveScope) -> usize {
    match scope {
        DriveScope::MetadataReadonly => 0,
        DriveScope::DriveReadonly => 1,
        DriveScope::Drive => 2,
    }
}

fn max_scope(left: DriveScope, right: DriveScope) -> DriveScope {
    if scope_rank(left) >= scope_rank(right) {
        left
    } else {
        right
    }
}

fn account_from_about(about: About) -> CoreResult<AccountProfile> {
    let user = about.user.ok_or_else(|| {
        CoreError::Message("Google Drive `about.get` did not include the authenticated user".into())
    })?;
    let account_id = user.permission_id.ok_or_else(|| {
        CoreError::Message("Google Drive user profile did not include a permissionId".into())
    })?;
    let email = user.email_address.ok_or_else(|| {
        CoreError::Message("Google Drive user profile did not include an email address".into())
    })?;
    Ok(AccountProfile { account_id, email, display_name: user.display_name })
}

fn map_file_list(page: google_drive3::api::FileList) -> CoreResult<FileListPage> {
    let files = page
        .files
        .unwrap_or_default()
        .into_iter()
        .map(map_drive_file)
        .collect::<CoreResult<_>>()?;
    Ok(FileListPage { next_page_token: page.next_page_token, files })
}

fn map_change_list(page: GoogleChangeList) -> CoreResult<ChangeListPage> {
    let mut removed_file_ids = Vec::new();
    let mut updated_files = Vec::new();
    for change in page.changes.unwrap_or_default() {
        apply_change(change, &mut removed_file_ids, &mut updated_files)?;
    }
    Ok(ChangeListPage {
        next_page_token: page.next_page_token,
        new_start_page_token: page.new_start_page_token,
        removed_file_ids,
        updated_files,
    })
}

fn apply_change(
    change: Change,
    removed_file_ids: &mut Vec<String>,
    updated_files: &mut Vec<FileRecord>,
) -> CoreResult<()> {
    if change.removed.unwrap_or(false) {
        if let Some(file_id) = change.file_id {
            removed_file_ids.push(file_id);
        }
        return Ok(());
    }
    if let Some(file) = change.file {
        updated_files.push(map_drive_file(file)?);
    }
    Ok(())
}

fn map_drive_file(file: GoogleFile) -> CoreResult<FileRecord> {
    let permissions = file
        .permissions
        .unwrap_or_default()
        .into_iter()
        .map(map_drive_permission)
        .collect::<Vec<_>>();
    let owned_by_me = file.owned_by_me.unwrap_or(false);
    let operator_can_share_manage =
        file.capabilities.as_ref().and_then(|capabilities| capabilities.can_share).unwrap_or(false)
            || owned_by_me;
    Ok(FileRecord {
        id: required_field(file.id, "file.id")?,
        name: required_field(file.name, "file.name")?,
        mime_type: required_field(file.mime_type, "file.mimeType")?,
        parents: file.parents.unwrap_or_default(),
        trashed: file.trashed.unwrap_or(false),
        owned_by_me,
        shared: file.shared.unwrap_or(!permissions.is_empty()),
        operator_can_share_manage,
        size: file.size.and_then(|value| u64::try_from(value).ok()),
        md5_checksum: file.md5_checksum,
        modified_time: file.modified_time,
        viewed_by_me_time: file.viewed_by_me_time,
        permissions,
        web_view_link: file.web_view_link,
        quota_bytes_used: file.quota_bytes_used.and_then(|value| u64::try_from(value).ok()),
        quota_bytes_total: None,
        image_media_metadata: file.image_media_metadata.map(map_image_media_metadata),
    })
}

fn map_drive_permission(permission: google_drive3::api::Permission) -> PermissionRecord {
    let inherited = permission
        .permission_details
        .as_ref()
        .and_then(|details| {
            if details.is_empty() {
                None
            } else {
                Some(details.iter().all(|detail| detail.inherited.unwrap_or(false)))
            }
        })
        .unwrap_or(false);
    let permission_type = permission.type_.unwrap_or_default();
    let role = permission.role.unwrap_or_default();
    PermissionRecord {
        id: permission.id.unwrap_or_default(),
        permission_type: permission_type.clone(),
        role: role.clone(),
        email_address: permission.email_address,
        domain: permission.domain,
        allow_file_discovery: permission.allow_file_discovery.unwrap_or(false),
        inherited,
        actionable: classify_permission_actionable(
            &permission_type,
            &role,
            inherited,
            permission.deleted.unwrap_or(false),
            permission.pending_owner.unwrap_or(false),
        ),
        display_name: permission.display_name,
    }
}

fn classify_permission_actionable(
    permission_type: &str,
    role: &str,
    inherited: bool,
    deleted: bool,
    pending_owner: bool,
) -> bool {
    if inherited || deleted || pending_owner {
        return false;
    }
    if matches!(role, "owner" | "organizer") {
        return false;
    }
    matches!(permission_type, "user" | "group" | "domain" | "anyone")
}

fn map_image_media_metadata(
    metadata: google_drive3::api::FileImageMediaMetadata,
) -> ImageMediaMetadata {
    ImageMediaMetadata {
        width: metadata.width.and_then(|value| u32::try_from(value).ok()),
        height: metadata.height.and_then(|value| u32::try_from(value).ok()),
        camera_make: metadata.camera_make,
        camera_model: metadata.camera_model,
        date_taken: metadata.time.as_deref().and_then(parse_drive_image_time),
        exposure_time: metadata.exposure_time.map(format_float),
        aperture: metadata.aperture.map(format_float),
        focal_length: metadata.focal_length.map(format_float),
        iso_speed: metadata.iso_speed.and_then(|value| u32::try_from(value).ok()),
    }
}

fn parse_drive_image_time(raw: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(raw).map(|value| value.with_timezone(&Utc)).ok().or_else(|| {
        NaiveDateTime::parse_from_str(raw, "%Y:%m:%d %H:%M:%S")
            .ok()
            .map(|value| DateTime::<Utc>::from_naive_utc_and_offset(value, Utc))
    })
}

fn format_float(value: f32) -> String {
    let mut rendered = format!("{value:.6}");
    while rendered.contains('.') && rendered.ends_with('0') {
        rendered.pop();
    }
    if rendered.ends_with('.') {
        rendered.pop();
    }
    rendered
}

fn required_field<T>(value: Option<T>, field_name: &str) -> CoreResult<T> {
    value.ok_or_else(|| {
        CoreError::Message(format!("Google Drive response did not include `{field_name}`"))
    })
}

fn map_token_error(error: Box<dyn std::error::Error + Send + Sync>) -> CoreError {
    if let Some(oauth_error) = error.downcast_ref::<yup_oauth2::Error>() {
        return CoreError::Message(render_oauth_error(oauth_error));
    }
    let message = error.to_string();
    if message.contains("invalid_grant") || message.contains("expired") {
        return CoreError::Message(format!(
            "stored Google OAuth session is revoked or expired: {message}"
        ));
    }
    CoreError::Message(format!("failed to obtain a Google OAuth token: {message}"))
}

fn render_oauth_error(error: &yup_oauth2::Error) -> String {
    match error {
        yup_oauth2::Error::AuthError(auth_error) => {
            let code = auth_error.error.as_str();
            if matches!(
                auth_error.error,
                yup_oauth2::error::AuthErrorCode::InvalidGrant
                    | yup_oauth2::error::AuthErrorCode::ExpiredToken
                    | yup_oauth2::error::AuthErrorCode::AccessDenied
            ) {
                format!("stored Google OAuth session is revoked or expired: {code}")
            } else {
                format!("Google OAuth authorization failed: {auth_error}")
            }
        }
        other => format!("Google OAuth failed: {other}"),
    }
}

fn map_google_error(context: &str, error: common::Error) -> CoreError {
    match error {
        common::Error::MissingToken(inner) => map_token_error(inner),
        common::Error::BadRequest(value) => {
            let message = extract_google_error_message(&value)
                .unwrap_or_else(|| format!("Google Drive rejected the request: {value}"));
            if message.to_ascii_lowercase().contains("page token") {
                CoreError::Message(format!(
                    "invalid page token from Google Drive changes feed: {message}"
                ))
            } else {
                CoreError::Message(message)
            }
        }
        common::Error::Failure(response) => {
            let status = response.status();
            if status.as_u16() == 410 {
                CoreError::Message(format!(
                    "410 Gone from Google Drive changes feed while {context}"
                ))
            } else {
                CoreError::Message(format!("{context} failed with HTTP status {status}"))
            }
        }
        other => CoreError::Message(format!("{context} failed: {other}")),
    }
}

fn extract_google_error_message(value: &Value) -> Option<String> {
    value
        .get("error")
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .or_else(|| value.get("message").and_then(Value::as_str).map(ToOwned::to_owned))
}

fn extract_revocable_tokens(path: &Path) -> CoreResult<Vec<String>> {
    let contents = std::fs::read_to_string(path)?;
    let value = serde_json::from_str::<Value>(&contents)?;
    let mut tokens = BTreeSet::new();
    collect_token_strings(&value, &mut tokens);
    Ok(tokens.into_iter().collect())
}

fn collect_token_strings(value: &Value, tokens: &mut BTreeSet<String>) {
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                if matches!(key.as_str(), "access_token" | "refresh_token") {
                    if let Some(token) = value.as_str() {
                        tokens.insert(token.to_string());
                    }
                }
                collect_token_strings(value, tokens);
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_token_strings(value, tokens);
            }
        }
        _ => {}
    }
}

mod google_live;

#[cfg(test)]
mod lib_tests;
