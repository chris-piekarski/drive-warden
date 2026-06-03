use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

pub const APP_NAME: &str = "gdrive-optimize";
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const GOOGLE_DRIVE_FOLDER_MIME: &str = "application/vnd.google-apps.folder";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DriveScope {
    MetadataReadonly,
    DriveReadonly,
    Drive,
}

impl DriveScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MetadataReadonly => "drive.metadata.readonly",
            Self::DriveReadonly => "drive.readonly",
            Self::Drive => "drive",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SyncMode {
    Full,
    Delta,
}

impl SyncMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Delta => "delta",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SyncStatus {
    Never,
    InProgress,
    Committed,
    Failed,
}

impl SyncStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Never => "never",
            Self::InProgress => "in_progress",
            Self::Committed => "committed",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PathState {
    Resolved,
    MultiParent,
    Orphaned,
}

impl PathState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Resolved => "resolved",
            Self::MultiParent => "multi_parent",
            Self::Orphaned => "orphaned",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountProfile {
    pub account_id: String,
    pub email: String,
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthSession {
    pub account: AccountProfile,
    pub active_scopes: Vec<DriveScope>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct AuthStatus {
    pub session: Option<AuthSession>,
}

impl AuthStatus {
    pub fn is_logged_in(&self) -> bool {
        self.session.is_some()
    }

    pub fn require_session(self) -> CoreResult<AuthSession> {
        self.session.ok_or_else(|| CoreError::Message("not logged in".into()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteFileMetadata {
    pub id: String,
    pub name: String,
    pub mime_type: String,
    pub size: Option<u64>,
    pub modified_time: Option<DateTime<Utc>>,
    pub owned_by_me: bool,
    pub shared: bool,
    pub permissions: Vec<PermissionRecord>,
}

impl From<FileRecord> for RemoteFileMetadata {
    fn from(file: FileRecord) -> Self {
        Self {
            id: file.id,
            name: file.name,
            mime_type: file.mime_type,
            size: file.size,
            modified_time: file.modified_time,
            owned_by_me: file.owned_by_me,
            shared: file.shared,
            permissions: file.permissions,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteDbManifest {
    pub version: u32,
    pub db_name: String,
    #[serde(default)]
    pub db_instance_id: Option<String>,
    #[serde(default)]
    pub db_generation: Option<i64>,
    pub sha256: String,
    pub byte_len: u64,
    pub uploaded_at: DateTime<Utc>,
    pub local_modified_time: Option<DateTime<Utc>>,
    #[serde(default)]
    pub sqlite_page_count: Option<u64>,
    #[serde(default)]
    pub sqlite_schema_version: Option<u32>,
    #[serde(default)]
    pub source_label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RemoteDbEndpoint {
    pub folder: Option<RemoteFileMetadata>,
    pub db_file: Option<RemoteFileMetadata>,
    pub manifest_file: Option<RemoteFileMetadata>,
    pub manifest: Option<RemoteDbManifest>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RemoteDbSyncDecision {
    PushLocal,
    PullRemote,
    NeedsExplicitDirection,
    NothingToSync,
}

impl RemoteDbSyncDecision {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PushLocal => "push_local",
            Self::PullRemote => "pull_remote",
            Self::NeedsExplicitDirection => "needs_explicit_direction",
            Self::NothingToSync => "nothing_to_sync",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemotePrivacyIssue {
    pub file_id: String,
    pub file_name: String,
    pub permission_id: String,
    pub permission_type: String,
    pub role: String,
    pub target_label: String,
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect::<String>()
}

pub fn build_remote_db_manifest(
    db_name: &str,
    bytes: &[u8],
    db_instance_id: Option<String>,
    db_generation: Option<i64>,
    local_modified_time: Option<DateTime<Utc>>,
    sqlite_page_count: Option<u64>,
    sqlite_schema_version: Option<u32>,
    source_label: Option<String>,
) -> RemoteDbManifest {
    RemoteDbManifest {
        version: 1,
        db_name: db_name.to_string(),
        db_instance_id,
        db_generation,
        sha256: sha256_hex(bytes),
        byte_len: bytes.len() as u64,
        uploaded_at: Utc::now(),
        local_modified_time,
        sqlite_page_count,
        sqlite_schema_version,
        source_label,
    }
}

pub fn verify_remote_db_manifest(manifest: &RemoteDbManifest, bytes: &[u8]) -> CoreResult<()> {
    let byte_len = bytes.len() as u64;
    if manifest.byte_len != byte_len {
        return Err(CoreError::Message(format!(
            "remote DB manifest byte length mismatch: manifest={} downloaded={byte_len}",
            manifest.byte_len
        )));
    }
    let actual = sha256_hex(bytes);
    if manifest.sha256 != actual {
        return Err(CoreError::Message(format!(
            "remote DB manifest checksum mismatch: manifest={} downloaded={actual}",
            manifest.sha256
        )));
    }
    Ok(())
}

pub fn decide_remote_db_sync(local_exists: bool, remote_exists: bool) -> RemoteDbSyncDecision {
    match (local_exists, remote_exists) {
        (true, false) => RemoteDbSyncDecision::PushLocal,
        (false, true) => RemoteDbSyncDecision::PullRemote,
        (true, true) => RemoteDbSyncDecision::NeedsExplicitDirection,
        (false, false) => RemoteDbSyncDecision::NothingToSync,
    }
}

pub fn validate_remote_db_privacy(files: &[RemoteFileMetadata]) -> Vec<RemotePrivacyIssue> {
    files
        .iter()
        .flat_map(|file| {
            let ownership_issue = (!file.owned_by_me).then(|| RemotePrivacyIssue {
                file_id: file.id.clone(),
                file_name: file.name.clone(),
                permission_id: "ownedByMe".into(),
                permission_type: "owner".into(),
                role: "owner".into(),
                target_label: "not owned by authenticated operator".into(),
            });
            ownership_issue.into_iter().chain(file.permissions.iter().filter_map(
                move |permission| {
                    if is_private_owner_permission(permission) {
                        return None;
                    }
                    Some(RemotePrivacyIssue {
                        file_id: file.id.clone(),
                        file_name: file.name.clone(),
                        permission_id: permission.id.clone(),
                        permission_type: permission.permission_type.clone(),
                        role: permission.role.clone(),
                        target_label: permission
                            .email_address
                            .clone()
                            .or_else(|| permission.domain.clone())
                            .or_else(|| permission.display_name.clone())
                            .unwrap_or_else(|| permission.permission_type.clone()),
                    })
                },
            ))
        })
        .collect()
}

fn is_private_owner_permission(permission: &PermissionRecord) -> bool {
    permission.permission_type == "user"
        && permission.role == "owner"
        && permission.email_address.is_some()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncState {
    pub account: AccountProfile,
    pub active_scopes: Vec<DriveScope>,
    pub committed_start_page_token: Option<String>,
    pub committed_generation: i64,
    pub last_sync_status: SyncStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncRun {
    pub run_id: String,
    pub mode: SyncMode,
    pub generation: i64,
    pub source_page_token: Option<String>,
    pub started_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct FileRecord {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub mime_type: String,
    #[serde(default)]
    pub parents: Vec<String>,
    #[serde(default)]
    pub trashed: bool,
    #[serde(default)]
    pub owned_by_me: bool,
    #[serde(default)]
    pub shared: bool,
    #[serde(default)]
    pub operator_can_share_manage: bool,
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default)]
    pub md5_checksum: Option<String>,
    #[serde(default)]
    pub modified_time: Option<DateTime<Utc>>,
    #[serde(default)]
    pub viewed_by_me_time: Option<DateTime<Utc>>,
    #[serde(default)]
    pub permissions: Vec<PermissionRecord>,
    #[serde(default)]
    pub web_view_link: Option<String>,
    #[serde(default)]
    pub quota_bytes_used: Option<u64>,
    #[serde(default)]
    pub quota_bytes_total: Option<u64>,
    #[serde(default)]
    pub image_media_metadata: Option<ImageMediaMetadata>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ImageMediaMetadata {
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
    #[serde(default)]
    pub camera_make: Option<String>,
    #[serde(default)]
    pub camera_model: Option<String>,
    #[serde(default)]
    pub date_taken: Option<DateTime<Utc>>,
    #[serde(default)]
    pub exposure_time: Option<String>,
    #[serde(default)]
    pub aperture: Option<String>,
    #[serde(default)]
    pub focal_length: Option<String>,
    #[serde(default)]
    pub iso_speed: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PermissionRecord {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub permission_type: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub email_address: Option<String>,
    #[serde(default)]
    pub domain: Option<String>,
    #[serde(default)]
    pub allow_file_discovery: bool,
    #[serde(default)]
    pub inherited: bool,
    #[serde(default)]
    pub actionable: bool,
    #[serde(default)]
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct FileListPage {
    pub next_page_token: Option<String>,
    pub files: Vec<FileRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ChangeListPage {
    pub next_page_token: Option<String>,
    pub new_start_page_token: Option<String>,
    pub removed_file_ids: Vec<String>,
    pub updated_files: Vec<FileRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct FullSnapshot {
    pub files: Vec<FileRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SyncStats {
    pub added: usize,
    pub updated: usize,
    pub removed: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathEntry {
    pub file_id: String,
    pub primary_path: String,
    pub all_paths: Vec<String>,
    pub depth: usize,
    pub path_state: PathState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InventoryItem {
    pub file: FileRecord,
    pub path: PathEntry,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncSummary {
    pub mode: SyncMode,
    pub added: usize,
    pub updated: usize,
    pub removed: usize,
    pub committed_page_token: String,
    pub generation: i64,
    pub file_count: usize,
    pub path_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OwnerScope {
    Mine,
    All,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SharedWithFilter {
    Anyone,
    Domain(String),
    Email(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InventoryQuery {
    pub name_contains: Option<String>,
    pub mime_contains: Option<String>,
    pub older_than_days: Option<i64>,
    pub larger_than: Option<u64>,
    pub in_folder: Option<String>,
    pub path_glob: Option<String>,
    pub shared_only: bool,
    pub shared_with: Option<SharedWithFilter>,
    pub owner_scope: OwnerScope,
    pub actionable_only: bool,
    pub duplicate_of: Option<String>,
    pub limit: Option<usize>,
    pub offset: usize,
}

impl Default for InventoryQuery {
    fn default() -> Self {
        Self {
            name_contains: None,
            mime_contains: None,
            older_than_days: None,
            larger_than: None,
            in_folder: None,
            path_glob: None,
            shared_only: false,
            shared_with: None,
            owner_scope: OwnerScope::All,
            actionable_only: false,
            duplicate_of: None,
            limit: None,
            offset: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DuplicateMatchType {
    Md5,
    NameSize,
}

impl DuplicateMatchType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Md5 => "md5",
            Self::NameSize => "name_size",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DuplicateGroup {
    pub group_key: String,
    pub match_type: DuplicateMatchType,
    pub items: Vec<InventoryItem>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SharingKind {
    Anyone,
    Domain,
    ExternalEmail,
    InternalEmail,
}

impl SharingKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Anyone => "anyone",
            Self::Domain => "domain",
            Self::ExternalEmail => "external_email",
            Self::InternalEmail => "internal_email",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharingFinding {
    pub item: InventoryItem,
    pub permission: PermissionRecord,
    pub kind: SharingKind,
    pub target_label: String,
    pub actionable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageFinding {
    pub item: InventoryItem,
    pub size_bytes: u64,
    pub stale_days: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageSummary {
    pub total_files: usize,
    pub total_bytes: u64,
    pub large_files: Vec<StorageFinding>,
    pub stale_files: Vec<StorageFinding>,
    pub stale_threshold_days: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InspectFileDetails {
    pub item: InventoryItem,
    pub duplicate_groups: Vec<DuplicateGroup>,
    pub sharing_findings: Vec<SharingFinding>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExifSource {
    DriveImageMediaMetadata,
    DownloadedBytes,
}

impl ExifSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DriveImageMediaMetadata => "drive_image_media_metadata",
            Self::DownloadedBytes => "downloaded_bytes",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InspectExifDetails {
    pub file_id: String,
    pub name: String,
    pub mime_type: String,
    pub web_view_link: Option<String>,
    pub source: ExifSource,
    pub metadata: ImageMediaMetadata,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UnshareReasonCode {
    Actionable,
    /// Actionable, but the grant lives on an operator-managed ancestor folder, so
    /// the delete is applied there once and cascades to this inherited child.
    ActionableViaFolder,
    InheritedPermission,
    /// Inherited from a folder owned by the grantee (operator cannot manage it);
    /// the only remedy is to move the item out of that folder.
    GranteeOwnedParent,
    NotActionable,
    NotOwnedOrManageable,
}

impl UnshareReasonCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Actionable => "actionable",
            Self::ActionableViaFolder => "actionable_via_folder",
            Self::InheritedPermission => "inherited_permission",
            Self::GranteeOwnedParent => "grantee_owned_parent",
            Self::NotActionable => "not_actionable",
            Self::NotOwnedOrManageable => "not_owned_or_manageable",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrashReasonCode {
    Actionable,
    NotOwnedOrManageable,
    FolderWithoutRecursive,
}

impl TrashReasonCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Actionable => "actionable",
            Self::NotOwnedOrManageable => "not_owned_or_manageable",
            Self::FolderWithoutRecursive => "folder_without_recursive",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnsharePreviewRow {
    pub item: InventoryItem,
    pub permission: PermissionRecord,
    pub kind: SharingKind,
    pub target_label: String,
    pub reason: UnshareReasonCode,
    pub actionable: bool,
    /// When set, the delete is applied to this ancestor folder's permission and
    /// cascades to `item` (folder-inherited grant). `None` => delete on `item` itself.
    #[serde(default)]
    pub apply_target_file_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct UnsharePlan {
    pub rows: Vec<UnsharePreviewRow>,
    pub actionable_count: usize,
    pub skipped_count: usize,
    pub public_count: usize,
    pub domain_count: usize,
    pub direct_count: usize,
    #[serde(default)]
    pub retain_copy: Option<RetainCopyPlan>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnshareApplySummary {
    pub planned: usize,
    pub applied: usize,
    pub skipped: usize,
    #[serde(default)]
    pub retain_copy: Option<RetainCopyApplySummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RetainCopyOptions {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub backup_root_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetainCopyPlanEntry {
    pub source_item: InventoryItem,
    pub descendant_file_count: usize,
    pub descendant_folder_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetainCopyPlan {
    pub destination_label: String,
    pub destination_parent_id: Option<String>,
    pub entries: Vec<RetainCopyPlanEntry>,
    pub root_count: usize,
    pub total_file_copies: usize,
    pub total_folder_copies: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetainCopyApplySummary {
    pub root_count: usize,
    pub copied_files: usize,
    pub created_folders: usize,
    pub destination_folder_id: String,
    pub destination_folder_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TrashOptions {
    #[serde(default)]
    pub recursive: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrashPreviewRow {
    pub item: InventoryItem,
    pub reason: TrashReasonCode,
    pub actionable: bool,
    pub descendant_file_count: usize,
    pub descendant_folder_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TrashPlan {
    pub rows: Vec<TrashPreviewRow>,
    pub actionable_count: usize,
    pub skipped_count: usize,
    pub file_count: usize,
    pub folder_count: usize,
    pub total_bytes: u64,
    pub recursive: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrashApplySummary {
    pub planned: usize,
    pub applied: usize,
    pub skipped: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditLogEntry {
    pub at: DateTime<Utc>,
    pub command: String,
    pub action: String,
    pub file_id: String,
    pub permission_id: String,
    pub target_label: String,
    pub dry_run: bool,
    #[serde(default)]
    pub source_file_id: Option<String>,
    #[serde(default)]
    pub backup_file_id: Option<String>,
}

/// Append-only snapshot of a sharing permission at the moment it was revoked.
///
/// `audit_log` records that a delete happened; this captures the richer metadata
/// (role, grantee type, file name/path, the source folder a cascade was applied
/// at) so the operator can audit *what access existed* after `sync` has rewritten
/// `files.permissions_json` to the post-revoke state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevokedShareEntry {
    pub at: DateTime<Utc>,
    pub command: String,
    pub file_id: String,
    pub file_name: String,
    pub file_path: String,
    pub grantee: String,
    pub grantee_type: String,
    pub role: String,
    pub permission_id: String,
    pub inherited: bool,
    #[serde(default)]
    pub source_folder_id: Option<String>,
    pub revoked_via: String,
    #[serde(default)]
    pub note: Option<String>,
}

/// Append-only snapshot of a file or folder at the moment it was moved to trash.
///
/// `audit_log` records each explicit Drive API trash call; this captures richer
/// metadata for every affected item, including descendants moved by recursive
/// folder trash, so operators can track recovery windows after sync removes
/// items from the active inventory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrashedFileEntry {
    pub at: DateTime<Utc>,
    #[serde(default)]
    pub recoverable_until: Option<DateTime<Utc>>,
    pub command: String,
    pub file_id: String,
    pub file_name: String,
    pub file_path: String,
    pub mime_type: String,
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default)]
    pub md5_checksum: Option<String>,
    #[serde(default)]
    pub modified_time: Option<DateTime<Utc>>,
    #[serde(default)]
    pub trashed_via_file_id: Option<String>,
    #[serde(default)]
    pub trashed_via_path: Option<String>,
    pub explicitly_requested: bool,
    pub descendant_file_count: usize,
    pub descendant_folder_count: usize,
    pub trash_via: String,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("{0}")]
    Message(String),
}

pub type CoreResult<T> = Result<T, CoreError>;

impl From<std::io::Error> for CoreError {
    fn from(value: std::io::Error) -> Self {
        Self::Message(value.to_string())
    }
}

impl From<serde_json::Error> for CoreError {
    fn from(value: serde_json::Error) -> Self {
        Self::Message(value.to_string())
    }
}

#[async_trait]
pub trait DriveGateway: Send + Sync {
    async fn login(&self, scope: DriveScope) -> CoreResult<AuthSession>;
    async fn logout(&self) -> CoreResult<bool>;
    async fn auth_status(&self) -> CoreResult<AuthStatus>;
    async fn list_files(&self, page_token: Option<&str>) -> CoreResult<FileListPage>;
    async fn get_start_page_token(&self) -> CoreResult<String>;
    async fn list_changes(&self, page_token: &str) -> CoreResult<ChangeListPage>;
    async fn get_file(&self, id: &str) -> CoreResult<FileRecord>;
    async fn inspect_exif(&self, id: &str) -> CoreResult<InspectExifDetails>;
    async fn ensure_scope(&self, scope: DriveScope) -> CoreResult<()>;
    async fn create_folder(&self, parent_id: &str, name: &str) -> CoreResult<FileRecord>;
    async fn copy_file(
        &self,
        file_id: &str,
        parent_id: &str,
        name: Option<&str>,
    ) -> CoreResult<FileRecord>;
    async fn delete_permission(&self, file_id: &str, permission_id: &str) -> CoreResult<()>;
    async fn trash_file(&self, file_id: &str) -> CoreResult<()> {
        Err(CoreError::Message(format!(
            "trash operation is not supported by this Drive backend for file `{file_id}`"
        )))
    }
    async fn find_file_in_folder(
        &self,
        parent_id: &str,
        name: &str,
    ) -> CoreResult<Option<RemoteFileMetadata>> {
        Err(CoreError::Message(format!(
            "remote file lookup is not supported by this Drive backend for `{name}` in `{parent_id}`"
        )))
    }
    async fn list_files_in_folder(
        &self,
        parent_id: &str,
        name_prefix: Option<&str>,
    ) -> CoreResult<Vec<RemoteFileMetadata>> {
        let _ = name_prefix;
        Err(CoreError::Message(format!(
            "remote folder listing is not supported by this Drive backend for `{parent_id}`"
        )))
    }
    async fn upload_file_to_folder(
        &self,
        parent_id: &str,
        name: &str,
        mime_type: &str,
        contents: Vec<u8>,
    ) -> CoreResult<RemoteFileMetadata> {
        let _ = contents;
        Err(CoreError::Message(format!(
            "remote file upload is not supported by this Drive backend for `{name}` in `{parent_id}` as `{mime_type}`"
        )))
    }
    async fn update_file_contents(
        &self,
        file_id: &str,
        name: &str,
        mime_type: &str,
        contents: Vec<u8>,
    ) -> CoreResult<RemoteFileMetadata> {
        let _ = contents;
        Err(CoreError::Message(format!(
            "remote file update is not supported by this Drive backend for `{file_id}` (`{name}` as `{mime_type}`)"
        )))
    }
    async fn download_file(&self, file_id: &str) -> CoreResult<Vec<u8>> {
        Err(CoreError::Message(format!(
            "remote file download is not supported by this Drive backend for `{file_id}`"
        )))
    }
}

pub trait InventoryRepository: Send + Sync {
    fn get_sync_state(&self) -> CoreResult<Option<SyncState>>;
    fn get_sync_state_for_account(&self, account_id: &str) -> CoreResult<Option<SyncState>> {
        Ok(self.get_sync_state()?.filter(|state| state.account.account_id == account_id))
    }
    fn load_snapshot(&self) -> CoreResult<FullSnapshot>;
    fn load_inventory_items(&self) -> CoreResult<Vec<InventoryItem>>;
    fn inspect_file(&self, id: &str) -> CoreResult<Option<InventoryItem>>;
    fn append_audit_log(&self, entry: &AuditLogEntry) -> CoreResult<()>;
    fn load_audit_log(&self) -> CoreResult<Vec<AuditLogEntry>>;
    /// Persist a revoked-share snapshot. Defaults to a no-op so in-memory/test
    /// repositories need not implement it; the SQLite repository overrides this.
    fn append_revoked_share(&self, entry: &RevokedShareEntry) -> CoreResult<()> {
        let _ = entry;
        Ok(())
    }
    fn load_revoked_shares(&self) -> CoreResult<Vec<RevokedShareEntry>> {
        Ok(Vec::new())
    }
    fn append_trashed_file(&self, entry: &TrashedFileEntry) -> CoreResult<()> {
        let _ = entry;
        Ok(())
    }
    fn load_trashed_files(&self) -> CoreResult<Vec<TrashedFileEntry>> {
        Ok(Vec::new())
    }
    fn begin_sync_run(
        &self,
        account: &AccountProfile,
        active_scopes: &[DriveScope],
        mode: SyncMode,
        source_page_token: Option<&str>,
    ) -> CoreResult<SyncRun>;
    fn replace_snapshot(
        &self,
        run: &SyncRun,
        account: &AccountProfile,
        active_scopes: &[DriveScope],
        snapshot: &FullSnapshot,
        committed_page_token: &str,
        stats: SyncStats,
    ) -> CoreResult<SyncSummary>;
    fn mark_sync_failed(&self, run: &SyncRun, message: &str) -> CoreResult<()>;
}

pub trait ReportWriter: Send + Sync {
    fn write_markdown(&self, path: &str, contents: &str) -> CoreResult<()>;
}

pub async fn login<G: DriveGateway + ?Sized>(
    gateway: &G,
    scope: DriveScope,
) -> CoreResult<AuthSession> {
    gateway.login(scope).await
}

pub async fn logout<G: DriveGateway + ?Sized>(gateway: &G) -> CoreResult<bool> {
    gateway.logout().await
}

pub async fn auth_status<G: DriveGateway + ?Sized>(gateway: &G) -> CoreResult<AuthStatus> {
    gateway.auth_status().await
}

pub fn inventory_items<R: InventoryRepository + ?Sized>(
    repository: &R,
    query: &InventoryQuery,
) -> CoreResult<Vec<InventoryItem>> {
    let items = repository.load_inventory_items()?;
    Ok(apply_inventory_query(items, query))
}

pub fn duplicate_groups<R: InventoryRepository + ?Sized>(
    repository: &R,
    query: &InventoryQuery,
) -> CoreResult<Vec<DuplicateGroup>> {
    let items = repository.load_inventory_items()?;
    Ok(apply_paging(build_duplicate_groups(items, query), query))
}

pub fn sharing_findings<R: InventoryRepository + ?Sized>(
    repository: &R,
    query: &InventoryQuery,
) -> CoreResult<Vec<SharingFinding>> {
    let state = repository.get_sync_state()?;
    let account_email = state.as_ref().map(|sync_state| sync_state.account.email.as_str());
    let items = repository.load_inventory_items()?;
    Ok(apply_paging(
        build_sharing_findings(apply_inventory_filters(items, query), account_email, query),
        query,
    ))
}

pub fn storage_summary<R: InventoryRepository + ?Sized>(
    repository: &R,
    query: &InventoryQuery,
    stale_threshold_days: i64,
) -> CoreResult<StorageSummary> {
    let items = apply_inventory_filters(repository.load_inventory_items()?, query);
    Ok(build_storage_summary(items, stale_threshold_days, query))
}

pub fn inspect_file_details<R: InventoryRepository + ?Sized>(
    repository: &R,
    id: &str,
) -> CoreResult<Option<InspectFileDetails>> {
    let Some(item) = repository.inspect_file(id)? else {
        return Ok(None);
    };

    let duplicate_query =
        InventoryQuery { duplicate_of: Some(id.to_string()), ..InventoryQuery::default() };
    let duplicate_groups = duplicate_groups(repository, &duplicate_query)?;
    let sharing_query = InventoryQuery::default();
    let sharing_findings = sharing_findings(repository, &sharing_query)?
        .into_iter()
        .filter(|finding| finding.item.file.id == id)
        .collect();

    Ok(Some(InspectFileDetails { item, duplicate_groups, sharing_findings }))
}

pub async fn inspect_exif<G: DriveGateway + ?Sized>(
    gateway: &G,
    id: &str,
) -> CoreResult<InspectExifDetails> {
    gateway.ensure_scope(DriveScope::DriveReadonly).await?;
    gateway.inspect_exif(id).await
}

pub fn unshare_plan<R: InventoryRepository + ?Sized>(
    repository: &R,
    query: &InventoryQuery,
) -> CoreResult<UnsharePlan> {
    unshare_plan_with_options(repository, query, None)
}

pub fn unshare_plan_with_options<R: InventoryRepository + ?Sized>(
    repository: &R,
    query: &InventoryQuery,
    retain_copy_options: Option<&RetainCopyOptions>,
) -> CoreResult<UnsharePlan> {
    let mut plan_query = query.clone();
    plan_query.actionable_only = false;
    let findings = sharing_findings(repository, &plan_query)?;

    // Full inventory powers parent-chain inherited detection (Google omits the
    // per-permission `inherited` flag for My Drive). `matched_ids` keeps cascade
    // revokes from over-revoking siblings the query did not select.
    let all_items = repository.load_inventory_items()?;
    let items_by_id: HashMap<String, &InventoryItem> =
        all_items.iter().map(|item| (item.file.id.clone(), item)).collect();
    let matched_ids: HashSet<String> =
        findings.iter().map(|finding| finding.item.file.id.clone()).collect();

    let mut plan = UnsharePlan::default();

    for finding in findings {
        let (reason, apply_target_file_id) =
            classify_unshare_finding(&finding, &matched_ids, &items_by_id);
        let actionable = matches!(
            reason,
            UnshareReasonCode::Actionable | UnshareReasonCode::ActionableViaFolder
        );

        match finding.kind {
            SharingKind::Anyone => plan.public_count += 1,
            SharingKind::Domain => plan.domain_count += 1,
            SharingKind::ExternalEmail | SharingKind::InternalEmail => plan.direct_count += 1,
        }

        if actionable {
            plan.actionable_count += 1;
        } else {
            plan.skipped_count += 1;
        }

        plan.rows.push(UnsharePreviewRow {
            item: finding.item,
            permission: finding.permission,
            kind: finding.kind,
            target_label: finding.target_label,
            reason,
            actionable,
            apply_target_file_id,
        });
    }

    if let Some(options) = retain_copy_options.filter(|options| options.enabled) {
        let items = repository.load_inventory_items()?;
        plan.retain_copy = Some(build_retain_copy_plan(&items, &plan.rows, options));
    }

    Ok(plan)
}

pub async fn apply_unshare<G, R>(
    gateway: &G,
    repository: &R,
    query: &InventoryQuery,
    command: &str,
) -> CoreResult<UnshareApplySummary>
where
    G: DriveGateway + ?Sized,
    R: InventoryRepository + ?Sized,
{
    apply_unshare_with_options(gateway, repository, query, None, command).await
}

pub async fn apply_unshare_with_options<G, R>(
    gateway: &G,
    repository: &R,
    query: &InventoryQuery,
    retain_copy_options: Option<&RetainCopyOptions>,
    command: &str,
) -> CoreResult<UnshareApplySummary>
where
    G: DriveGateway + ?Sized,
    R: InventoryRepository + ?Sized,
{
    ensure_committed_snapshot(repository)?;
    let plan = unshare_plan_with_options(repository, query, retain_copy_options)?;
    if plan.actionable_count == 0 {
        return Ok(UnshareApplySummary {
            planned: plan.rows.len(),
            applied: 0,
            skipped: plan.rows.len(),
            retain_copy: None,
        });
    }

    gateway.ensure_scope(DriveScope::Drive).await?;

    let retain_copy = if let Some(retain_copy_plan) = &plan.retain_copy {
        let items = repository.load_inventory_items()?;
        Some(apply_retain_copy_plan(gateway, repository, &items, retain_copy_plan, command).await?)
    } else {
        None
    };

    let mut applied = 0usize;
    // A folder-inherited grant is removed once at the source folder; that single
    // delete cascades to every inherited child, so dedupe the API call while still
    // recording history for each affected file.
    let mut deleted_targets: BTreeSet<(String, String)> = BTreeSet::new();
    for row in &plan.rows {
        if !row.actionable {
            continue;
        }

        let target_file_id =
            row.apply_target_file_id.clone().unwrap_or_else(|| row.item.file.id.clone());
        let permission_id = row.permission.id.clone();
        if deleted_targets.insert((target_file_id.clone(), permission_id.clone())) {
            repository.append_audit_log(&AuditLogEntry {
                at: Utc::now(),
                command: command.to_string(),
                action: "delete_permission_pending".into(),
                file_id: row.item.file.id.clone(),
                permission_id: permission_id.clone(),
                target_label: row.target_label.clone(),
                dry_run: false,
                source_file_id: row.apply_target_file_id.clone(),
                backup_file_id: None,
            })?;
            gateway.delete_permission(&target_file_id, &permission_id).await?;
        }

        let now = Utc::now();
        repository.append_audit_log(&AuditLogEntry {
            at: now,
            command: command.to_string(),
            action: "delete_permission".into(),
            file_id: row.item.file.id.clone(),
            permission_id: permission_id.clone(),
            target_label: row.target_label.clone(),
            dry_run: false,
            source_file_id: row.apply_target_file_id.clone(),
            backup_file_id: None,
        })?;
        repository.append_revoked_share(&RevokedShareEntry {
            at: now,
            command: command.to_string(),
            file_id: row.item.file.id.clone(),
            file_name: row.item.file.name.clone(),
            file_path: row.item.path.primary_path.clone(),
            grantee: row.target_label.clone(),
            grantee_type: row.permission.permission_type.clone(),
            role: row.permission.role.clone(),
            permission_id,
            inherited: row.apply_target_file_id.is_some() || row.permission.inherited,
            source_folder_id: row.apply_target_file_id.clone(),
            revoked_via: "tool".into(),
            note: None,
        })?;
        applied += 1;
    }

    Ok(UnshareApplySummary {
        planned: plan.rows.len(),
        applied,
        skipped: plan.rows.len().saturating_sub(applied),
        retain_copy,
    })
}

pub fn trash_plan<R: InventoryRepository + ?Sized>(
    repository: &R,
    query: &InventoryQuery,
    options: &TrashOptions,
) -> CoreResult<TrashPlan> {
    let items = repository.load_inventory_items()?;
    let selected_items = apply_inventory_query(items.clone(), query);
    Ok(build_trash_plan(&items, selected_items, options))
}

fn ensure_committed_snapshot<R: InventoryRepository + ?Sized>(repository: &R) -> CoreResult<()> {
    let Some(state) = repository.get_sync_state()? else {
        return Err(CoreError::Message(
            "destructive apply requires a committed local sync snapshot; run `gdrive-optimize sync` first"
                .into(),
        ));
    };
    if state.last_sync_status != SyncStatus::Committed {
        return Err(CoreError::Message(format!(
            "destructive apply requires a committed local sync snapshot; last sync status is `{}`",
            state.last_sync_status.as_str()
        )));
    }
    Ok(())
}

pub async fn apply_trash<G, R>(
    gateway: &G,
    repository: &R,
    query: &InventoryQuery,
    options: &TrashOptions,
    command: &str,
) -> CoreResult<TrashApplySummary>
where
    G: DriveGateway + ?Sized,
    R: InventoryRepository + ?Sized,
{
    ensure_committed_snapshot(repository)?;
    let plan = trash_plan(repository, query, options)?;
    if plan.actionable_count == 0 {
        return Ok(TrashApplySummary {
            planned: plan.rows.len(),
            applied: 0,
            skipped: plan.rows.len(),
        });
    }

    gateway.ensure_scope(DriveScope::Drive).await?;
    let all_items = repository.load_inventory_items()?;
    let items_by_id = all_items
        .iter()
        .cloned()
        .map(|item| (item.file.id.clone(), item))
        .collect::<HashMap<_, _>>();
    let children_by_parent = build_children_by_parent(&all_items);

    let mut applied = 0usize;
    for row in &plan.rows {
        if !row.actionable {
            continue;
        }

        let now = Utc::now();
        repository.append_audit_log(&AuditLogEntry {
            at: now,
            command: command.to_string(),
            action: "trash_file_pending".into(),
            file_id: row.item.file.id.clone(),
            permission_id: String::new(),
            target_label: row.item.path.primary_path.clone(),
            dry_run: false,
            source_file_id: None,
            backup_file_id: None,
        })?;
        gateway.trash_file(&row.item.file.id).await?;
        let now = Utc::now();
        repository.append_audit_log(&AuditLogEntry {
            at: now,
            command: command.to_string(),
            action: "trash_file".into(),
            file_id: row.item.file.id.clone(),
            permission_id: String::new(),
            target_label: row.item.path.primary_path.clone(),
            dry_run: false,
            source_file_id: None,
            backup_file_id: None,
        })?;
        for entry in
            build_trashed_file_entries(row, &items_by_id, &children_by_parent, now, command)
        {
            repository.append_trashed_file(&entry)?;
        }
        applied += 1;
    }

    Ok(TrashApplySummary {
        planned: plan.rows.len(),
        applied,
        skipped: plan.rows.len().saturating_sub(applied),
    })
}

fn build_trashed_file_entries(
    row: &TrashPreviewRow,
    items_by_id: &HashMap<String, InventoryItem>,
    children_by_parent: &HashMap<String, Vec<String>>,
    at: DateTime<Utc>,
    command: &str,
) -> Vec<TrashedFileEntry> {
    let recoverable_until = Some(at + Duration::days(30));
    let mut entries = Vec::new();
    let subtree_ids = if row.item.file.mime_type == GOOGLE_DRIVE_FOLDER_MIME {
        collect_subtree_ids(&row.item.file.id, children_by_parent)
    } else {
        BTreeSet::from([row.item.file.id.clone()])
    };

    for file_id in subtree_ids {
        let Some(item) = items_by_id.get(&file_id) else {
            continue;
        };
        let explicitly_requested = file_id == row.item.file.id;
        entries.push(TrashedFileEntry {
            at,
            recoverable_until,
            command: command.to_string(),
            file_id: item.file.id.clone(),
            file_name: item.file.name.clone(),
            file_path: item.path.primary_path.clone(),
            mime_type: item.file.mime_type.clone(),
            size: item.file.size,
            md5_checksum: item.file.md5_checksum.clone(),
            modified_time: item.file.modified_time,
            trashed_via_file_id: (!explicitly_requested).then(|| row.item.file.id.clone()),
            trashed_via_path: (!explicitly_requested).then(|| row.item.path.primary_path.clone()),
            explicitly_requested,
            descendant_file_count: if explicitly_requested { row.descendant_file_count } else { 0 },
            descendant_folder_count: if explicitly_requested {
                row.descendant_folder_count
            } else {
                0
            },
            trash_via: "tool".into(),
            note: None,
        });
    }
    entries
}

fn build_trash_plan(
    all_items: &[InventoryItem],
    selected_items: Vec<InventoryItem>,
    options: &TrashOptions,
) -> TrashPlan {
    let items_by_id = all_items
        .iter()
        .cloned()
        .map(|item| (item.file.id.clone(), item))
        .collect::<HashMap<_, _>>();
    let children_by_parent = build_children_by_parent(all_items);
    let selected_ids =
        selected_items.iter().map(|item| item.file.id.clone()).collect::<BTreeSet<_>>();
    let suppressed_descendants = if options.recursive {
        selected_items
            .iter()
            .filter(|item| item.file.operator_can_share_manage)
            .filter(|item| item.file.mime_type == GOOGLE_DRIVE_FOLDER_MIME)
            .flat_map(|item| {
                collect_subtree_ids(&item.file.id, &children_by_parent)
                    .into_iter()
                    .filter(|id| id != &item.file.id)
                    .collect::<Vec<_>>()
            })
            .filter(|id| selected_ids.contains(id))
            .collect::<BTreeSet<_>>()
    } else {
        BTreeSet::new()
    };

    let mut plan = TrashPlan { recursive: options.recursive, ..TrashPlan::default() };
    for item in selected_items {
        if suppressed_descendants.contains(&item.file.id) {
            continue;
        }

        let is_folder = item.file.mime_type == GOOGLE_DRIVE_FOLDER_MIME;
        let subtree_ids = if is_folder {
            collect_subtree_ids(&item.file.id, &children_by_parent)
        } else {
            BTreeSet::new()
        };
        let (descendant_file_count, descendant_folder_count, descendant_bytes) =
            count_descendants(&items_by_id, &subtree_ids, &item.file.id);
        let reason = classify_trash_reason(&item, options);
        let actionable = matches!(reason, TrashReasonCode::Actionable);

        if is_folder {
            plan.folder_count += 1;
        } else {
            plan.file_count += 1;
        }
        if actionable {
            plan.actionable_count += 1;
            plan.total_bytes += item.file.size.unwrap_or(0) + descendant_bytes;
        } else {
            plan.skipped_count += 1;
        }

        plan.rows.push(TrashPreviewRow {
            item,
            reason,
            actionable,
            descendant_file_count,
            descendant_folder_count,
        });
    }

    plan
}

fn classify_trash_reason(item: &InventoryItem, options: &TrashOptions) -> TrashReasonCode {
    if !item.file.operator_can_share_manage {
        TrashReasonCode::NotOwnedOrManageable
    } else if item.file.mime_type == GOOGLE_DRIVE_FOLDER_MIME && !options.recursive {
        TrashReasonCode::FolderWithoutRecursive
    } else {
        TrashReasonCode::Actionable
    }
}

fn count_descendants(
    items_by_id: &HashMap<String, InventoryItem>,
    subtree_ids: &BTreeSet<String>,
    root_id: &str,
) -> (usize, usize, u64) {
    let mut file_count = 0usize;
    let mut folder_count = 0usize;
    let mut bytes = 0u64;
    for subtree_id in subtree_ids {
        if subtree_id == root_id {
            continue;
        }
        let Some(item) = items_by_id.get(subtree_id) else {
            continue;
        };
        if item.file.mime_type == GOOGLE_DRIVE_FOLDER_MIME {
            folder_count += 1;
        } else {
            file_count += 1;
        }
        bytes += item.file.size.unwrap_or(0);
    }
    (file_count, folder_count, bytes)
}

fn build_retain_copy_plan(
    items: &[InventoryItem],
    rows: &[UnsharePreviewRow],
    options: &RetainCopyOptions,
) -> RetainCopyPlan {
    let items_by_id =
        items.iter().cloned().map(|item| (item.file.id.clone(), item)).collect::<HashMap<_, _>>();
    let children_by_parent = build_children_by_parent(items);
    let root_candidates = unique_actionable_roots(rows);
    let folder_subtrees = root_candidates
        .iter()
        .filter_map(|root_id| {
            items_by_id.get(root_id).and_then(|item| {
                if item.file.mime_type == GOOGLE_DRIVE_FOLDER_MIME {
                    Some((root_id.clone(), collect_subtree_ids(root_id, &children_by_parent)))
                } else {
                    None
                }
            })
        })
        .collect::<Vec<_>>();

    let mut entries = Vec::new();
    let mut total_file_copies = 0usize;
    let mut total_folder_copies = 0usize;
    for root_id in root_candidates {
        let Some(source_item) = items_by_id.get(&root_id).cloned() else {
            continue;
        };
        let suppressed_by_ancestor = folder_subtrees
            .iter()
            .any(|(ancestor_id, subtree)| ancestor_id != &root_id && subtree.contains(&root_id));
        if suppressed_by_ancestor {
            continue;
        }

        let subtree_ids = collect_subtree_ids(&root_id, &children_by_parent);
        let mut descendant_file_count = 0usize;
        let mut descendant_folder_count = 0usize;
        for subtree_id in &subtree_ids {
            let Some(item) = items_by_id.get(subtree_id) else {
                continue;
            };
            if item.file.mime_type == GOOGLE_DRIVE_FOLDER_MIME {
                descendant_folder_count += 1;
            } else {
                descendant_file_count += 1;
            }
        }

        total_file_copies += descendant_file_count;
        total_folder_copies += descendant_folder_count;
        entries.push(RetainCopyPlanEntry {
            source_item,
            descendant_file_count,
            descendant_folder_count,
        });
    }

    RetainCopyPlan {
        destination_label: render_retain_copy_destination_label(options),
        destination_parent_id: options.backup_root_id.clone(),
        root_count: entries.len(),
        entries,
        total_file_copies,
        total_folder_copies,
    }
}

async fn apply_retain_copy_plan<G, R>(
    gateway: &G,
    repository: &R,
    items: &[InventoryItem],
    plan: &RetainCopyPlan,
    command: &str,
) -> CoreResult<RetainCopyApplySummary>
where
    G: DriveGateway + ?Sized,
    R: InventoryRepository + ?Sized,
{
    let destination_parent_id = plan.destination_parent_id.as_deref().unwrap_or("root");
    let destination_folder_name =
        format!("gdrive-optimize retained copy {}", Utc::now().format("%Y%m%dT%H%M%SZ"));
    let destination_folder =
        gateway.create_folder(destination_parent_id, &destination_folder_name).await?;
    repository.append_audit_log(&AuditLogEntry {
        at: Utc::now(),
        command: command.to_string(),
        action: "create_backup_folder".into(),
        file_id: destination_folder.id.clone(),
        permission_id: String::new(),
        target_label: plan.destination_label.clone(),
        dry_run: false,
        source_file_id: None,
        backup_file_id: Some(destination_folder.id.clone()),
    })?;

    let items_by_id =
        items.iter().cloned().map(|item| (item.file.id.clone(), item)).collect::<HashMap<_, _>>();
    let children_by_parent = build_children_by_parent(items);
    let mut created_folders = 1usize;
    let mut copied_files = 0usize;

    for entry in &plan.entries {
        if entry.source_item.file.mime_type == GOOGLE_DRIVE_FOLDER_MIME {
            let created =
                gateway.create_folder(&destination_folder.id, &entry.source_item.file.name).await?;
            repository.append_audit_log(&AuditLogEntry {
                at: Utc::now(),
                command: command.to_string(),
                action: "create_backup_folder".into(),
                file_id: entry.source_item.file.id.clone(),
                permission_id: String::new(),
                target_label: entry.source_item.path.primary_path.clone(),
                dry_run: false,
                source_file_id: Some(entry.source_item.file.id.clone()),
                backup_file_id: Some(created.id.clone()),
            })?;
            created_folders += 1;
            let counts = copy_folder_children(
                gateway,
                repository,
                &items_by_id,
                &children_by_parent,
                &entry.source_item.file.id,
                &created.id,
                command,
                &mut BTreeSet::from([entry.source_item.file.id.clone()]),
            )
            .await?;
            created_folders += counts.0;
            copied_files += counts.1;
        } else {
            let copied = gateway
                .copy_file(
                    &entry.source_item.file.id,
                    &destination_folder.id,
                    Some(&entry.source_item.file.name),
                )
                .await?;
            repository.append_audit_log(&AuditLogEntry {
                at: Utc::now(),
                command: command.to_string(),
                action: "copy_backup_file".into(),
                file_id: entry.source_item.file.id.clone(),
                permission_id: String::new(),
                target_label: entry.source_item.path.primary_path.clone(),
                dry_run: false,
                source_file_id: Some(entry.source_item.file.id.clone()),
                backup_file_id: Some(copied.id.clone()),
            })?;
            copied_files += 1;
        }
    }

    Ok(RetainCopyApplySummary {
        root_count: plan.root_count,
        copied_files,
        created_folders,
        destination_folder_id: destination_folder.id,
        destination_folder_name,
    })
}

async fn copy_folder_children<G, R>(
    gateway: &G,
    repository: &R,
    items_by_id: &HashMap<String, InventoryItem>,
    children_by_parent: &HashMap<String, Vec<String>>,
    source_folder_id: &str,
    destination_folder_id: &str,
    command: &str,
    visited: &mut BTreeSet<String>,
) -> CoreResult<(usize, usize)>
where
    G: DriveGateway + ?Sized,
    R: InventoryRepository + ?Sized,
{
    let mut created_folders = 0usize;
    let mut copied_files = 0usize;
    let mut children = children_by_parent.get(source_folder_id).cloned().unwrap_or_default();
    children.sort();

    for child_id in children {
        if !visited.insert(child_id.clone()) {
            continue;
        }
        let Some(child) = items_by_id.get(&child_id) else {
            continue;
        };
        if child.file.mime_type == GOOGLE_DRIVE_FOLDER_MIME {
            let created = gateway.create_folder(destination_folder_id, &child.file.name).await?;
            repository.append_audit_log(&AuditLogEntry {
                at: Utc::now(),
                command: command.to_string(),
                action: "create_backup_folder".into(),
                file_id: child.file.id.clone(),
                permission_id: String::new(),
                target_label: child.path.primary_path.clone(),
                dry_run: false,
                source_file_id: Some(child.file.id.clone()),
                backup_file_id: Some(created.id.clone()),
            })?;
            created_folders += 1;
            let nested = Box::pin(copy_folder_children(
                gateway,
                repository,
                items_by_id,
                children_by_parent,
                &child.file.id,
                &created.id,
                command,
                visited,
            ))
            .await?;
            created_folders += nested.0;
            copied_files += nested.1;
        } else {
            let copied = gateway
                .copy_file(&child.file.id, destination_folder_id, Some(&child.file.name))
                .await?;
            repository.append_audit_log(&AuditLogEntry {
                at: Utc::now(),
                command: command.to_string(),
                action: "copy_backup_file".into(),
                file_id: child.file.id.clone(),
                permission_id: String::new(),
                target_label: child.path.primary_path.clone(),
                dry_run: false,
                source_file_id: Some(child.file.id.clone()),
                backup_file_id: Some(copied.id.clone()),
            })?;
            copied_files += 1;
        }
    }

    Ok((created_folders, copied_files))
}

fn unique_actionable_roots(rows: &[UnsharePreviewRow]) -> Vec<String> {
    let mut roots = Vec::new();
    let mut seen = BTreeSet::new();
    for row in rows {
        if row.actionable && seen.insert(row.item.file.id.clone()) {
            roots.push(row.item.file.id.clone());
        }
    }
    roots
}

fn build_children_by_parent(items: &[InventoryItem]) -> HashMap<String, Vec<String>> {
    let mut children_by_parent = HashMap::<String, Vec<String>>::new();
    for item in items {
        for parent_id in &item.file.parents {
            children_by_parent.entry(parent_id.clone()).or_default().push(item.file.id.clone());
        }
    }
    children_by_parent
}

fn collect_subtree_ids(
    root_id: &str,
    children_by_parent: &HashMap<String, Vec<String>>,
) -> BTreeSet<String> {
    let mut collected = BTreeSet::new();
    let mut pending = vec![root_id.to_string()];
    while let Some(current) = pending.pop() {
        if !collected.insert(current.clone()) {
            continue;
        }
        if let Some(children) = children_by_parent.get(&current) {
            for child in children {
                pending.push(child.clone());
            }
        }
    }
    collected
}

fn render_retain_copy_destination_label(options: &RetainCopyOptions) -> String {
    options
        .backup_root_id
        .as_ref()
        .map(|root_id| {
            format!("Google Drive folder `{root_id}` with an auto-created retained-copy subfolder")
        })
        .unwrap_or_else(|| "My Drive root with an auto-created retained-copy folder".into())
}

pub async fn sync_inventory<G, R>(
    gateway: &G,
    repository: &R,
    force_full: bool,
) -> CoreResult<SyncSummary>
where
    G: DriveGateway + ?Sized,
    R: InventoryRepository + ?Sized,
{
    gateway.ensure_scope(DriveScope::MetadataReadonly).await?;
    let session = gateway.auth_status().await?.require_session()?;
    let state = repository.get_sync_state_for_account(&session.account.account_id)?;
    let source_page_token = state.as_ref().and_then(|item| item.committed_start_page_token.clone());

    match (force_full, source_page_token) {
        (true, _) | (_, None) => run_full_sync(gateway, repository, &session).await,
        (false, Some(source_page_token)) => {
            match run_delta_sync(gateway, repository, &session, source_page_token).await {
                Ok(summary) => Ok(summary),
                Err(error) if is_expired_delta_token_error(&error) => {
                    run_full_sync(gateway, repository, &session).await
                }
                Err(error) => Err(error),
            }
        }
    }
}

fn is_expired_delta_token_error(error: &CoreError) -> bool {
    let message = error.to_string().to_ascii_lowercase();
    message.contains("410 gone")
        || (message.contains("invalid") && message.contains("page token"))
        || (message.contains("expired") && message.contains("page token"))
}

async fn run_full_sync<G, R>(
    gateway: &G,
    repository: &R,
    session: &AuthSession,
) -> CoreResult<SyncSummary>
where
    G: DriveGateway + ?Sized,
    R: InventoryRepository + ?Sized,
{
    let run = repository.begin_sync_run(
        &session.account,
        &session.active_scopes,
        SyncMode::Full,
        None,
    )?;
    let result = async {
        let checkpoint_token = gateway.get_start_page_token().await?;
        let snapshot = collect_full_snapshot(gateway).await?;
        let (snapshot, committed_page_token) =
            apply_changes_until_stable(gateway, snapshot, checkpoint_token).await?;

        repository.replace_snapshot(
            &run,
            &session.account,
            &session.active_scopes,
            &snapshot,
            &committed_page_token,
            SyncStats { added: snapshot.files.len(), updated: 0, removed: 0 },
        )
    }
    .await;

    if let Err(error) = &result {
        let _ = repository.mark_sync_failed(&run, &error.to_string());
    }

    result
}

async fn run_delta_sync<G, R>(
    gateway: &G,
    repository: &R,
    session: &AuthSession,
    source_page_token: String,
) -> CoreResult<SyncSummary>
where
    G: DriveGateway + ?Sized,
    R: InventoryRepository + ?Sized,
{
    let run = repository.begin_sync_run(
        &session.account,
        &session.active_scopes,
        SyncMode::Delta,
        Some(&source_page_token),
    )?;
    let result = async {
        let snapshot = repository.load_snapshot()?;
        let (snapshot, stats, committed_page_token) =
            apply_delta_to_snapshot(gateway, snapshot, source_page_token).await?;

        repository.replace_snapshot(
            &run,
            &session.account,
            &session.active_scopes,
            &snapshot,
            &committed_page_token,
            stats,
        )
    }
    .await;

    if let Err(error) = &result {
        let _ = repository.mark_sync_failed(&run, &error.to_string());
    }

    result
}

async fn collect_full_snapshot<G>(gateway: &G) -> CoreResult<FullSnapshot>
where
    G: DriveGateway + ?Sized,
{
    let mut files = Vec::new();
    let mut next_page_token = None;

    loop {
        let page = gateway.list_files(next_page_token.as_deref()).await?;
        files.extend(page.files);

        match page.next_page_token {
            Some(token) => next_page_token = Some(token),
            None => break,
        }
    }

    Ok(FullSnapshot { files })
}

async fn apply_changes_until_stable<G>(
    gateway: &G,
    snapshot: FullSnapshot,
    start_page_token: String,
) -> CoreResult<(FullSnapshot, String)>
where
    G: DriveGateway + ?Sized,
{
    let (snapshot, _, committed_page_token) =
        apply_delta_to_snapshot(gateway, snapshot, start_page_token.clone()).await?;

    Ok((snapshot, committed_page_token))
}

async fn apply_delta_to_snapshot<G>(
    gateway: &G,
    snapshot: FullSnapshot,
    source_page_token: String,
) -> CoreResult<(FullSnapshot, SyncStats, String)>
where
    G: DriveGateway + ?Sized,
{
    let mut current_page_token = source_page_token.clone();
    let mut working = snapshot;
    let mut stats = SyncStats::default();

    loop {
        let page = gateway.list_changes(&current_page_token).await?;
        apply_change_page(&mut working, &page, &mut stats);

        if let Some(next_page_token) = page.next_page_token {
            current_page_token = next_page_token;
            continue;
        }

        let committed_page_token = page.new_start_page_token.unwrap_or(current_page_token);
        return Ok((working, stats, committed_page_token));
    }
}

pub fn apply_change_page(
    snapshot: &mut FullSnapshot,
    page: &ChangeListPage,
    stats: &mut SyncStats,
) {
    let mut files_by_id =
        snapshot.files.drain(..).map(|file| (file.id.clone(), file)).collect::<BTreeMap<_, _>>();

    for removed_id in &page.removed_file_ids {
        if files_by_id.remove(removed_id).is_some() {
            stats.removed += 1;
        }
    }

    for file in &page.updated_files {
        if file.trashed {
            if files_by_id.remove(&file.id).is_some() {
                stats.removed += 1;
            }
            continue;
        }

        if files_by_id.insert(file.id.clone(), file.clone()).is_some() {
            stats.updated += 1;
        } else {
            stats.added += 1;
        }
    }

    snapshot.files = files_by_id.into_values().collect();
}

pub fn build_path_entries(snapshot: &FullSnapshot) -> Vec<PathEntry> {
    let files = snapshot
        .files
        .iter()
        .cloned()
        .map(|file| (file.id.clone(), file))
        .collect::<BTreeMap<_, _>>();
    let mut cache = BTreeMap::<String, Vec<String>>::new();
    let mut orphan_cache = BTreeMap::<String, bool>::new();

    snapshot
        .files
        .iter()
        .map(|file| {
            let (all_paths, had_orphan) = resolve_paths_for_file(
                &files,
                &mut cache,
                &mut orphan_cache,
                &file.id,
                &mut BTreeSet::new(),
            );

            let path_state = if all_paths.len() > 1 {
                PathState::MultiParent
            } else if had_orphan {
                PathState::Orphaned
            } else {
                PathState::Resolved
            };
            let primary_path =
                all_paths.first().cloned().unwrap_or_else(|| format!("[orphan]/{}", file.name));
            let depth = primary_path.split('/').filter(|segment| !segment.is_empty()).count();

            PathEntry { file_id: file.id.clone(), primary_path, all_paths, depth, path_state }
        })
        .collect()
}

pub fn build_inventory_items(snapshot: &FullSnapshot) -> Vec<InventoryItem> {
    let path_entries = build_path_entries(snapshot)
        .into_iter()
        .map(|entry| (entry.file_id.clone(), entry))
        .collect::<HashMap<_, _>>();

    snapshot
        .files
        .iter()
        .filter_map(|file| {
            path_entries
                .get(&file.id)
                .cloned()
                .map(|path| InventoryItem { file: file.clone(), path })
        })
        .collect()
}

fn apply_inventory_query(items: Vec<InventoryItem>, query: &InventoryQuery) -> Vec<InventoryItem> {
    apply_paging(apply_inventory_filters(items, query), query)
}

fn apply_inventory_filters(
    items: Vec<InventoryItem>,
    query: &InventoryQuery,
) -> Vec<InventoryItem> {
    items.into_iter().filter(|item| inventory_item_matches_query(item, query)).collect()
}

fn inventory_item_matches_query(item: &InventoryItem, query: &InventoryQuery) -> bool {
    if matches!(query.owner_scope, OwnerScope::Mine) && !item.file.owned_by_me {
        return false;
    }
    if query.actionable_only && !item.file.operator_can_share_manage {
        return false;
    }
    if query.shared_only && !item.file.shared {
        return false;
    }
    if let Some(name_contains) = &query.name_contains {
        if !contains_ignore_case(&item.file.name, name_contains) {
            return false;
        }
    }
    if let Some(mime_contains) = &query.mime_contains {
        if !contains_ignore_case(&item.file.mime_type, mime_contains) {
            return false;
        }
    }
    if let Some(larger_than) = query.larger_than {
        if item.file.size.unwrap_or(0) < larger_than {
            return false;
        }
    }
    if let Some(older_than_days) = query.older_than_days {
        let Some(reference_time) = item.file.viewed_by_me_time.or(item.file.modified_time) else {
            return false;
        };
        let age_days = (Utc::now() - reference_time).num_days();
        if age_days < older_than_days {
            return false;
        }
    }
    if let Some(in_folder) = &query.in_folder {
        if !item.file.parents.iter().any(|parent| parent == in_folder) {
            return false;
        }
    }
    if let Some(path_glob) = &query.path_glob {
        if !simple_glob_match(path_glob, &item.path.primary_path) {
            return false;
        }
    }
    if let Some(shared_with) = &query.shared_with {
        let state = permission_matches_filter(&item.file.permissions, shared_with);
        if !state {
            return false;
        }
    }

    true
}

fn build_duplicate_groups(
    items: Vec<InventoryItem>,
    query: &InventoryQuery,
) -> Vec<DuplicateGroup> {
    let filtered_items = items
        .into_iter()
        .filter(|item| inventory_item_matches_query(item, query))
        .collect::<Vec<_>>();
    let mut md5_groups = BTreeMap::<String, Vec<InventoryItem>>::new();
    let mut name_size_groups = BTreeMap::<String, Vec<InventoryItem>>::new();

    for item in filtered_items {
        if let Some(md5_checksum) = &item.file.md5_checksum {
            md5_groups.entry(md5_checksum.clone()).or_default().push(item);
        } else if let Some(size) = item.file.size {
            let key = format!("{}::{size}", item.file.name.to_lowercase());
            name_size_groups.entry(key).or_default().push(item);
        }
    }

    let mut groups = Vec::new();
    for (key, items) in md5_groups {
        if items.len() > 1 {
            groups.push(DuplicateGroup {
                group_key: key,
                match_type: DuplicateMatchType::Md5,
                items,
            });
        }
    }
    for (key, items) in name_size_groups {
        if items.len() > 1 {
            groups.push(DuplicateGroup {
                group_key: key,
                match_type: DuplicateMatchType::NameSize,
                items,
            });
        }
    }

    if let Some(duplicate_of) = &query.duplicate_of {
        groups.retain(|group| group.items.iter().any(|item| item.file.id == *duplicate_of));
    }

    groups.sort_by(|left, right| left.group_key.cmp(&right.group_key));
    groups
}

fn build_sharing_findings(
    items: Vec<InventoryItem>,
    account_email: Option<&str>,
    query: &InventoryQuery,
) -> Vec<SharingFinding> {
    let account_domain = account_email.and_then(|email| email.split('@').nth(1)).map(str::to_owned);
    let mut findings = Vec::new();

    for item in items {
        for permission in &item.file.permissions {
            let kind = match permission.permission_type.as_str() {
                "anyone" => Some(SharingKind::Anyone),
                "domain" => Some(SharingKind::Domain),
                "user" | "group" => match (&permission.email_address, &account_domain) {
                    (Some(email), Some(domain)) if email.ends_with(&format!("@{domain}")) => {
                        Some(SharingKind::InternalEmail)
                    }
                    (Some(_), _) => Some(SharingKind::ExternalEmail),
                    _ => None,
                },
                _ => None,
            };

            let Some(kind) = kind else {
                continue;
            };

            let target_label = permission_target_label(permission);
            if let Some(shared_with) = &query.shared_with {
                let matching = match shared_with {
                    SharedWithFilter::Anyone => kind == SharingKind::Anyone,
                    SharedWithFilter::Domain(domain) => {
                        kind == SharingKind::Domain
                            && permission
                                .domain
                                .as_deref()
                                .map(|value| value.eq_ignore_ascii_case(domain))
                                .unwrap_or(false)
                    }
                    SharedWithFilter::Email(email) => permission
                        .email_address
                        .as_deref()
                        .map(|value| value.eq_ignore_ascii_case(email))
                        .unwrap_or(false),
                };
                if !matching {
                    continue;
                }
            }

            if query.actionable_only && !permission.actionable {
                continue;
            }

            findings.push(SharingFinding {
                item: item.clone(),
                permission: permission.clone(),
                kind,
                target_label,
                actionable: permission.actionable && item.file.operator_can_share_manage,
            });
        }
    }

    findings
}

fn classify_unshare_reason(finding: &SharingFinding) -> UnshareReasonCode {
    if finding.permission.inherited {
        UnshareReasonCode::InheritedPermission
    } else if !finding.item.file.operator_can_share_manage {
        UnshareReasonCode::NotOwnedOrManageable
    } else if !finding.permission.actionable {
        UnshareReasonCode::NotActionable
    } else {
        UnshareReasonCode::Actionable
    }
}

/// Stable identity for a permission's grantee, used to recognise the same person/
/// domain/anyone-link across a folder and the children that inherit its grant.
fn permission_grantee_key(permission: &PermissionRecord) -> Option<String> {
    match permission.permission_type.as_str() {
        "anyone" => Some("anyone".to_string()),
        "domain" => permission
            .domain
            .as_deref()
            .map(|domain| format!("domain:{}", domain.to_ascii_lowercase())),
        "user" | "group" => permission
            .email_address
            .as_deref()
            .map(|email| format!("email:{}", email.to_ascii_lowercase())),
        _ => None,
    }
}

fn file_carries_grantee(file: &FileRecord, grantee_key: &str) -> bool {
    file.permissions
        .iter()
        .any(|permission| permission_grantee_key(permission).as_deref() == Some(grantee_key))
}

/// Walk up the parent chain to the topmost ancestor reachable through a contiguous
/// run of folders that all carry `grantee_key`. That ancestor is where Google
/// granted the access; deleting the grant there cascades to every descendant.
/// Returns the file itself when no ancestor carries the grantee (a direct grant).
fn resolve_share_source<'a>(
    file_id: &str,
    grantee_key: &str,
    items_by_id: &HashMap<String, &'a InventoryItem>,
) -> Option<&'a InventoryItem> {
    let mut current = *items_by_id.get(file_id)?;
    for _ in 0..256 {
        let next = current.file.parents.iter().find_map(|parent_id| {
            items_by_id
                .get(parent_id)
                .copied()
                .filter(|parent| file_carries_grantee(&parent.file, grantee_key))
        });
        match next {
            Some(parent) => current = parent,
            None => break,
        }
    }
    Some(current)
}

/// Classify a finding with full inventory context so folder-inherited grants are
/// recognised even when Google did not populate the per-permission `inherited`
/// flag (which it omits for My Drive). Returns the reason plus, for cascade
/// revokes, the ancestor folder id the delete must be applied to.
fn classify_unshare_finding(
    finding: &SharingFinding,
    matched_ids: &HashSet<String>,
    items_by_id: &HashMap<String, &InventoryItem>,
) -> (UnshareReasonCode, Option<String>) {
    let Some(grantee_key) = permission_grantee_key(&finding.permission) else {
        return (classify_unshare_reason(finding), None);
    };
    let Some(source) = resolve_share_source(&finding.item.file.id, &grantee_key, items_by_id)
    else {
        return (classify_unshare_reason(finding), None);
    };

    if source.file.id == finding.item.file.id {
        // Direct grant on the file itself: existing semantics apply.
        return (classify_unshare_reason(finding), None);
    }

    // Grant lives on an ancestor folder (the access is inherited).
    if !source.file.operator_can_share_manage {
        // e.g. the item sits inside a folder the grantee owns — unrevokable here.
        return (UnshareReasonCode::GranteeOwnedParent, None);
    }
    if !matched_ids.contains(&source.file.id) {
        // The query did not select the source folder, so cascading would over-revoke
        // siblings. Surface as inherited; the operator should target the folder.
        return (UnshareReasonCode::InheritedPermission, None);
    }
    (UnshareReasonCode::ActionableViaFolder, Some(source.file.id.clone()))
}

fn build_storage_summary(
    items: Vec<InventoryItem>,
    stale_threshold_days: i64,
    query: &InventoryQuery,
) -> StorageSummary {
    let mut large_files = items
        .iter()
        .filter_map(|item| {
            item.file.size.map(|size_bytes| StorageFinding {
                item: item.clone(),
                size_bytes,
                stale_days: item
                    .file
                    .modified_time
                    .map(|modified| (Utc::now() - modified).num_days()),
            })
        })
        .collect::<Vec<_>>();
    large_files.sort_by_key(|item| std::cmp::Reverse(item.size_bytes));

    let mut stale_files = items
        .iter()
        .filter_map(|item| {
            let reference_time = item.file.viewed_by_me_time.or(item.file.modified_time)?;
            let stale_days = (Utc::now() - reference_time).num_days();
            if stale_days < stale_threshold_days {
                return None;
            }
            Some(StorageFinding {
                item: item.clone(),
                size_bytes: item.file.size.unwrap_or(0),
                stale_days: Some(stale_days),
            })
        })
        .collect::<Vec<_>>();
    stale_files.sort_by_key(|item| std::cmp::Reverse(item.stale_days.unwrap_or(0)));

    let total_bytes = items.iter().map(|item| item.file.size.unwrap_or(0)).sum();
    StorageSummary {
        total_files: items.len(),
        total_bytes,
        large_files: apply_paging(large_files, query),
        stale_files: apply_paging(stale_files, query),
        stale_threshold_days,
    }
}

fn permission_matches_filter(permissions: &[PermissionRecord], filter: &SharedWithFilter) -> bool {
    permissions.iter().any(|permission| match filter {
        SharedWithFilter::Anyone => permission.permission_type == "anyone",
        SharedWithFilter::Domain(domain) => {
            permission.permission_type == "domain"
                && permission
                    .domain
                    .as_deref()
                    .map(|value| value.eq_ignore_ascii_case(domain))
                    .unwrap_or(false)
        }
        SharedWithFilter::Email(email) => permission
            .email_address
            .as_deref()
            .map(|value| value.eq_ignore_ascii_case(email))
            .unwrap_or(false),
    })
}

fn permission_target_label(permission: &PermissionRecord) -> String {
    match permission.permission_type.as_str() {
        "anyone" => {
            if permission.allow_file_discovery {
                "anyone (discoverable)".to_string()
            } else {
                "anyone with link".to_string()
            }
        }
        "domain" => {
            format!("domain:{}", permission.domain.clone().unwrap_or_else(|| "unknown".into()))
        }
        "user" => permission.email_address.clone().unwrap_or_else(|| "user:unknown".into()),
        "group" => permission.email_address.clone().unwrap_or_else(|| "group:unknown".into()),
        _ => permission.permission_type.clone(),
    }
}

fn apply_paging<T>(items: Vec<T>, query: &InventoryQuery) -> Vec<T> {
    let iter = items.into_iter().skip(query.offset);
    match query.limit {
        Some(limit) => iter.take(limit).collect(),
        None => iter.collect(),
    }
}

fn contains_ignore_case(haystack: &str, needle: &str) -> bool {
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

fn simple_glob_match(pattern: &str, value: &str) -> bool {
    simple_glob_match_inner(
        &pattern.chars().collect::<Vec<_>>(),
        &value.chars().collect::<Vec<_>>(),
        0,
        0,
    )
}

fn simple_glob_match_inner(pattern: &[char], value: &[char], p: usize, v: usize) -> bool {
    if p == pattern.len() {
        return v == value.len();
    }

    match pattern[p] {
        '*' => {
            (v..=value.len()).any(|next_v| simple_glob_match_inner(pattern, value, p + 1, next_v))
        }
        '?' => v < value.len() && simple_glob_match_inner(pattern, value, p + 1, v + 1),
        literal => {
            v < value.len()
                && literal == value[v]
                && simple_glob_match_inner(pattern, value, p + 1, v + 1)
        }
    }
}

fn resolve_paths_for_file(
    files: &BTreeMap<String, FileRecord>,
    cache: &mut BTreeMap<String, Vec<String>>,
    orphan_cache: &mut BTreeMap<String, bool>,
    file_id: &str,
    visiting: &mut BTreeSet<String>,
) -> (Vec<String>, bool) {
    if let Some(paths) = cache.get(file_id) {
        return (paths.clone(), orphan_cache.get(file_id).copied().unwrap_or(false));
    }

    let Some(file) = files.get(file_id) else {
        return (vec![format!("[orphan]/{file_id}")], true);
    };

    if !visiting.insert(file_id.to_string()) {
        return (vec![format!("[orphan]/{}", file.name)], true);
    }

    let mut resolved_paths = BTreeSet::new();
    let mut had_orphan = false;

    if file.parents.is_empty() {
        resolved_paths.insert(format!("/{}", file.name));
    } else {
        for parent_id in &file.parents {
            if parent_id == "root" {
                resolved_paths.insert(format!("/{}", file.name));
                continue;
            }

            if !files.contains_key(parent_id) {
                had_orphan = true;
                continue;
            }

            let (parent_paths, parent_had_orphan) =
                resolve_paths_for_file(files, cache, orphan_cache, parent_id, visiting);
            had_orphan |= parent_had_orphan;

            for parent_path in parent_paths {
                let next_path = if parent_path == "/" {
                    format!("/{}", file.name)
                } else {
                    format!("{parent_path}/{}", file.name)
                };
                resolved_paths.insert(next_path);
            }
        }
    }

    visiting.remove(file_id);

    if resolved_paths.is_empty() {
        had_orphan = true;
        resolved_paths.insert(format!("[orphan]/{}", file.name));
    }

    let paths = resolved_paths.into_iter().collect::<Vec<_>>();
    cache.insert(file_id.to_string(), paths.clone());
    orphan_cache.insert(file_id.to_string(), had_orphan);
    (paths, had_orphan)
}

#[cfg(test)]
mod lib_tests;
