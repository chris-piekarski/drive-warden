use super::*;

use std::future::Future;
use std::time::Duration;

use google_drive3::{hyper_rustls, hyper_util, DriveHub};
use reqwest::Client;
use rustls::crypto::{self, aws_lc_rs};
use tokio::time::{sleep, timeout};

const GOOGLE_REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
const GOOGLE_REQUEST_MAX_ATTEMPTS: usize = 4;
const GOOGLE_RETRY_BASE_DELAY: Duration = Duration::from_millis(500);

macro_rules! google_request {
    ($context:expr, $body:block) => {
        execute_google_request($context, || async { $body }).await
    };
}

impl GoogleDriveGateway {
    pub(super) async fn perform_login(
        &self,
        requested_scope: DriveScope,
        existing: Option<&GoogleSessionState>,
    ) -> CoreResult<AuthSession> {
        if !self.credentials_path.exists() {
            return Err(CoreError::Message(format!(
                "Google OAuth credentials were not found at `{}`; configure a Desktop OAuth client credentials file and retry",
                self.credentials_path.display()
            )));
        }

        let hub = self.build_hub().await?;
        let scope_urls = oauth_scope_urls(requested_scope);
        hub.auth.get_token(&scope_urls).await.map_err(map_token_error)?;
        ensure_private_file_permissions(&self.token_path)?;

        let (_, about) = google_request!("fetching authenticated Google profile", {
            hub.about()
                .get()
                .add_scopes(scope_urls.clone())
                .param("fields", ABOUT_FIELDS)
                .doit()
                .await
        })?;

        let account = account_from_about(about)?;
        if let Some(existing) = existing {
            if existing.account.account_id != account.account_id {
                return Err(CoreError::Message(format!(
                    "scope upgrade completed for `{}`, but the existing local session is for `{}`; local credentials were left unchanged",
                    account.email, existing.account.email
                )));
            }
        }

        let active_scopes = cumulative_scopes(requested_scope);
        let session =
            AuthSession { account: account.clone(), active_scopes: active_scopes.clone() };
        self.write_google_session_state(&GoogleSessionState {
            version: 1,
            account,
            active_scopes,
            credentials_path: self.credentials_path.display().to_string(),
            token_path: self.token_path.display().to_string(),
        })?;
        Ok(session)
    }

    pub(super) async fn build_hub(&self) -> CoreResult<DriveHub<impl common::Connector>> {
        ensure_parent_dir(&self.token_path)?;
        let secret =
            yup_oauth2::read_application_secret(&self.credentials_path).await.map_err(|error| {
                CoreError::Message(format!(
                    "failed to read Google OAuth credentials from `{}`: {error}",
                    self.credentials_path.display()
                ))
            })?;
        let auth = yup_oauth2::InstalledFlowAuthenticator::builder(
            secret,
            yup_oauth2::InstalledFlowReturnMethod::HTTPRedirect,
        )
        .persist_tokens_to_disk(&self.token_path)
        .build()
        .await
        .map_err(|error| {
            CoreError::Message(format!("failed to initialize Google OAuth flow: {error}"))
        })?;
        Ok(DriveHub::new(build_http_client()?, auth))
    }

    pub(super) async fn revoke_stored_tokens(&self) {
        let Ok(tokens) = extract_revocable_tokens(&self.token_path) else {
            return;
        };
        let client = Client::new();
        for token in tokens {
            if let Ok(url) = reqwest::Url::parse_with_params(
                "https://oauth2.googleapis.com/revoke",
                &[("token", token.as_str())],
            ) {
                let _ = client.post(url).send().await;
            }
        }
    }
}

#[async_trait]
impl DriveGateway for GoogleDriveGateway {
    async fn login(&self, scope: DriveScope) -> CoreResult<AuthSession> {
        let existing = self.load_google_session_state()?;
        let target_scope = Self::highest_scope_from_session(existing.as_ref(), scope);
        self.perform_login(target_scope, existing.as_ref()).await
    }

    async fn logout(&self) -> CoreResult<bool> {
        let removed = self.session_path.exists() || self.token_path.exists();
        self.revoke_stored_tokens().await;
        let _ = delete_file_if_exists(&self.session_path)?;
        let _ = delete_file_if_exists(&self.token_path)?;
        Ok(removed)
    }

    async fn auth_status(&self) -> CoreResult<AuthStatus> {
        Ok(AuthStatus { session: self.read_session()? })
    }

    async fn list_files(&self, page_token: Option<&str>) -> CoreResult<FileListPage> {
        let session = self.ensure_scope_internal(DriveScope::MetadataReadonly).await?;
        let hub = self.build_hub().await?;
        let (_, page) = google_request!("listing Google Drive files", {
            let mut call = hub
                .files()
                .list()
                .add_scopes(oauth_scope_urls(highest_active_scope(&session.active_scopes)))
                .supports_all_drives(false)
                .include_items_from_all_drives(false)
                .spaces("drive")
                .corpora("user")
                .page_size(FILE_PAGE_SIZE)
                .include_permissions_for_view("published")
                .q("trashed = false")
                .param("fields", FILE_FIELDS);
            if let Some(page_token) = page_token {
                call = call.page_token(page_token);
            }
            call.doit().await
        })?;
        Ok(map_file_list(page)?)
    }

    async fn get_start_page_token(&self) -> CoreResult<String> {
        let session = self.ensure_scope_internal(DriveScope::MetadataReadonly).await?;
        let hub = self.build_hub().await?;
        let (_, response) = google_request!("fetching Google Drive start page token", {
            hub.changes()
                .get_start_page_token()
                .add_scopes(oauth_scope_urls(highest_active_scope(&session.active_scopes)))
                .supports_all_drives(false)
                .param("fields", "startPageToken")
                .doit()
                .await
        })?;
        response.start_page_token.ok_or_else(|| {
            CoreError::Message(
                "Google Drive did not return a startPageToken for the current account".into(),
            )
        })
    }

    async fn list_changes(&self, page_token: &str) -> CoreResult<ChangeListPage> {
        let session = self.ensure_scope_internal(DriveScope::MetadataReadonly).await?;
        let hub = self.build_hub().await?;
        let (_, page) = google_request!("listing Google Drive changes", {
            hub.changes()
                .list(page_token)
                .add_scopes(oauth_scope_urls(highest_active_scope(&session.active_scopes)))
                .supports_all_drives(false)
                .include_items_from_all_drives(false)
                .restrict_to_my_drive(true)
                .include_removed(true)
                .spaces("drive")
                .page_size(FILE_PAGE_SIZE)
                .include_permissions_for_view("published")
                .param("fields", CHANGE_FIELDS)
                .doit()
                .await
        })?;
        Ok(map_change_list(page)?)
    }

    async fn get_file(&self, id: &str) -> CoreResult<FileRecord> {
        let session = self.ensure_scope_internal(DriveScope::MetadataReadonly).await?;
        let hub = self.build_hub().await?;
        let context = format!("fetching Google Drive file `{id}`");
        let (_, file) = google_request!(&context, {
            hub.files()
                .get(id)
                .add_scopes(oauth_scope_urls(highest_active_scope(&session.active_scopes)))
                .supports_all_drives(false)
                .include_permissions_for_view("published")
                .param("fields", FILE_ITEM_FIELDS)
                .doit()
                .await
        })?;
        map_drive_file(file)
    }

    async fn inspect_exif(&self, id: &str) -> CoreResult<InspectExifDetails> {
        let session = self.ensure_scope_internal(DriveScope::DriveReadonly).await?;
        let hub = self.build_hub().await?;
        let context = format!("fetching Google Drive file `{id}`");
        let (_, file) = google_request!(&context, {
            hub.files()
                .get(id)
                .add_scopes(oauth_scope_urls(highest_active_scope(&session.active_scopes)))
                .supports_all_drives(false)
                .include_permissions_for_view("published")
                .param("fields", FILE_ITEM_FIELDS)
                .doit()
                .await
        })?;
        inspect_exif_details_from_record(id, map_drive_file(file)?)
    }

    async fn ensure_scope(&self, scope: DriveScope) -> CoreResult<()> {
        self.ensure_scope_internal(scope).await.map(|_| ())
    }

    async fn create_folder(&self, parent_id: &str, name: &str) -> CoreResult<FileRecord> {
        let session = self.ensure_scope_internal(DriveScope::Drive).await?;
        let hub = self.build_hub().await?;
        let request = create_folder_request(parent_id, name);
        let context = format!("creating Google Drive folder `{name}` under `{parent_id}`");
        let (_, folder) = google_request!(&context, {
            hub.files()
                .create(request.clone())
                .add_scopes(oauth_scope_urls(highest_active_scope(&session.active_scopes)))
                .supports_all_drives(false)
                .enforce_single_parent(true)
                .param("fields", FILE_ITEM_FIELDS)
                .upload(
                    std::io::Cursor::new(Vec::<u8>::new()),
                    "application/octet-stream"
                        .parse()
                        .expect("static octet-stream mime should parse"),
                )
                .await
        })?;
        map_drive_file(folder)
    }

    async fn copy_file(
        &self,
        file_id: &str,
        parent_id: &str,
        name: Option<&str>,
    ) -> CoreResult<FileRecord> {
        let session = self.ensure_scope_internal(DriveScope::Drive).await?;
        let hub = self.build_hub().await?;
        let request = copy_file_request(parent_id, name);
        let context = format!("copying Google Drive file `{file_id}` into `{parent_id}`");
        let (_, copied) = google_request!(&context, {
            hub.files()
                .copy(request.clone(), file_id)
                .add_scopes(oauth_scope_urls(highest_active_scope(&session.active_scopes)))
                .supports_all_drives(false)
                .enforce_single_parent(true)
                .param("fields", FILE_ITEM_FIELDS)
                .doit()
                .await
        })?;
        map_drive_file(copied)
    }

    async fn delete_permission(&self, file_id: &str, permission_id: &str) -> CoreResult<()> {
        let session = self.ensure_scope_internal(DriveScope::Drive).await?;
        let hub = self.build_hub().await?;
        let context = format!("deleting permission `{permission_id}` from file `{file_id}`");
        google_request!(&context, {
            hub.permissions()
                .delete(file_id, permission_id)
                .add_scopes(oauth_scope_urls(highest_active_scope(&session.active_scopes)))
                .supports_all_drives(false)
                .enforce_expansive_access(false)
                .doit()
                .await
        })?;
        Ok(())
    }

    async fn trash_file(&self, file_id: &str) -> CoreResult<()> {
        let session = self.ensure_scope_internal(DriveScope::Drive).await?;
        let hub = self.build_hub().await?;
        let request = trash_file_request();
        let context = format!("moving Google Drive file `{file_id}` to trash");
        google_request!(&context, {
            hub.files()
                .update(request.clone(), file_id)
                .add_scopes(oauth_scope_urls(highest_active_scope(&session.active_scopes)))
                .supports_all_drives(false)
                .param("fields", "id,trashed")
                .upload(
                    std::io::Cursor::new(Vec::<u8>::new()),
                    "application/octet-stream"
                        .parse()
                        .expect("static octet-stream mime should parse"),
                )
                .await
        })?;
        Ok(())
    }

    async fn find_file_in_folder(
        &self,
        parent_id: &str,
        name: &str,
    ) -> CoreResult<Option<RemoteFileMetadata>> {
        let session = self.ensure_scope_internal(DriveScope::MetadataReadonly).await?;
        let hub = self.build_hub().await?;
        let query = drive_find_file_query(parent_id, name);
        let context = format!("finding Google Drive file `{name}` in folder `{parent_id}`");
        let (_, page) = google_request!(&context, {
            hub.files()
                .list()
                .add_scopes(oauth_scope_urls(highest_active_scope(&session.active_scopes)))
                .supports_all_drives(false)
                .include_items_from_all_drives(false)
                .spaces("drive")
                .corpora("user")
                .page_size(10)
                .include_permissions_for_view("published")
                .q(&query)
                .param("fields", FILE_FIELDS)
                .doit()
                .await
        })?;
        let Some(file) = page.files.unwrap_or_default().into_iter().next() else {
            return Ok(None);
        };
        Ok(Some(RemoteFileMetadata::from(map_drive_file(file)?)))
    }

    async fn list_files_in_folder(
        &self,
        parent_id: &str,
        name_prefix: Option<&str>,
    ) -> CoreResult<Vec<RemoteFileMetadata>> {
        let session = self.ensure_scope_internal(DriveScope::MetadataReadonly).await?;
        let hub = self.build_hub().await?;
        let query = drive_folder_listing_query(parent_id, name_prefix);
        let mut page_token = None;
        let mut files = Vec::new();
        loop {
            let mut call = hub
                .files()
                .list()
                .add_scopes(oauth_scope_urls(highest_active_scope(&session.active_scopes)))
                .supports_all_drives(false)
                .include_items_from_all_drives(false)
                .spaces("drive")
                .corpora("user")
                .page_size(FILE_PAGE_SIZE)
                .include_permissions_for_view("published")
                .q(&query)
                .param("fields", FILE_FIELDS);
            if let Some(token) = page_token.as_deref() {
                call = call.page_token(token);
            }
            let context = format!("listing Google Drive files in folder `{parent_id}`");
            let token = page_token.clone();
            let (_, page) = google_request!(&context, {
                let mut call = hub
                    .files()
                    .list()
                    .add_scopes(oauth_scope_urls(highest_active_scope(&session.active_scopes)))
                    .supports_all_drives(false)
                    .include_items_from_all_drives(false)
                    .spaces("drive")
                    .corpora("user")
                    .page_size(FILE_PAGE_SIZE)
                    .include_permissions_for_view("published")
                    .q(&query)
                    .param("fields", FILE_FIELDS);
                if let Some(token) = token.as_deref() {
                    call = call.page_token(token);
                }
                call.doit().await
            })?;
            for file in page.files.unwrap_or_default() {
                files.push(RemoteFileMetadata::from(map_drive_file(file)?));
            }
            page_token = page.next_page_token;
            if page_token.is_none() {
                break;
            }
        }
        Ok(filter_remote_files_by_prefix(files, name_prefix))
    }

    async fn upload_file_to_folder(
        &self,
        parent_id: &str,
        name: &str,
        mime_type: &str,
        contents: Vec<u8>,
    ) -> CoreResult<RemoteFileMetadata> {
        let session = self.ensure_scope_internal(DriveScope::Drive).await?;
        let hub = self.build_hub().await?;
        let request = upload_file_request(parent_id, name, mime_type);
        let parsed_mime: mime::Mime = mime_type.parse().map_err(|error| {
            CoreError::Message(format!("invalid MIME type `{mime_type}`: {error}"))
        })?;
        let context = format!("uploading Google Drive file `{name}` into `{parent_id}`");
        let (_, file) = google_request!(&context, {
            hub.files()
                .create(request.clone())
                .add_scopes(oauth_scope_urls(highest_active_scope(&session.active_scopes)))
                .supports_all_drives(false)
                .enforce_single_parent(true)
                .param("fields", FILE_ITEM_FIELDS)
                .upload(std::io::Cursor::new(contents.clone()), parsed_mime.clone())
                .await
        })?;
        Ok(RemoteFileMetadata::from(map_drive_file(file)?))
    }

    async fn update_file_contents(
        &self,
        file_id: &str,
        name: &str,
        mime_type: &str,
        contents: Vec<u8>,
    ) -> CoreResult<RemoteFileMetadata> {
        let session = self.ensure_scope_internal(DriveScope::Drive).await?;
        let hub = self.build_hub().await?;
        let request = update_file_request(name, mime_type);
        let parsed_mime: mime::Mime = mime_type.parse().map_err(|error| {
            CoreError::Message(format!("invalid MIME type `{mime_type}`: {error}"))
        })?;
        let context = format!("updating Google Drive file `{file_id}`");
        let (_, file) = google_request!(&context, {
            hub.files()
                .update(request.clone(), file_id)
                .add_scopes(oauth_scope_urls(highest_active_scope(&session.active_scopes)))
                .supports_all_drives(false)
                .param("fields", FILE_ITEM_FIELDS)
                .upload(std::io::Cursor::new(contents.clone()), parsed_mime.clone())
                .await
        })?;
        Ok(RemoteFileMetadata::from(map_drive_file(file)?))
    }

    async fn rename_file(&self, file_id: &str, new_name: &str) -> CoreResult<RemoteFileMetadata> {
        let session = self.ensure_scope_internal(DriveScope::Drive).await?;
        let hub = self.build_hub().await?;
        let request = rename_file_request(new_name);
        let context = format!("renaming Google Drive file `{file_id}` to `{new_name}`");
        let (_, file) = google_request!(&context, {
            hub.files()
                .update(request.clone(), file_id)
                .add_scopes(oauth_scope_urls(highest_active_scope(&session.active_scopes)))
                .supports_all_drives(false)
                .param("fields", FILE_ITEM_FIELDS)
                .upload(
                    std::io::Cursor::new(Vec::<u8>::new()),
                    "application/octet-stream"
                        .parse()
                        .expect("static octet-stream mime should parse"),
                )
                .await
        })?;
        Ok(RemoteFileMetadata::from(map_drive_file(file)?))
    }

    async fn move_file(
        &self,
        file_id: &str,
        add_parent_id: &str,
        remove_parent_ids: &[String],
    ) -> CoreResult<RemoteFileMetadata> {
        let session = self.ensure_scope_internal(DriveScope::Drive).await?;
        let hub = self.build_hub().await?;
        let remove_parents = remove_parent_ids.join(",");
        let context = format!("moving Google Drive file `{file_id}` into folder `{add_parent_id}`");
        let scope_urls = oauth_scope_urls(highest_active_scope(&session.active_scopes));
        let access_token =
            hub.auth.get_token(&scope_urls).await.map_err(map_token_error)?.ok_or_else(|| {
                CoreError::Message("Google OAuth token missing access token".into())
            })?;
        let client = Client::new();
        let url = reqwest::Url::parse_with_params(
            &format!("https://www.googleapis.com/drive/v3/files/{file_id}"),
            &[
                ("addParents", add_parent_id),
                ("removeParents", remove_parents.as_str()),
                ("supportsAllDrives", "false"),
                ("fields", FILE_ITEM_FIELDS),
            ],
        )
        .map_err(|error| CoreError::Message(format!("{context} URL was invalid: {error}")))?;
        let request = || {
            client
                .patch(url.clone())
                .bearer_auth(access_token.clone())
                .header("content-type", "application/json")
                .body("{}")
        };
        let file = execute_reqwest_json_request::<GoogleFile, _>(&context, request).await?;
        Ok(RemoteFileMetadata::from(map_drive_file(file)?))
    }

    async fn download_file(&self, file_id: &str) -> CoreResult<Vec<u8>> {
        let session = self.ensure_scope_internal(DriveScope::DriveReadonly).await?;
        let hub = self.build_hub().await?;
        let context = format!("downloading Google Drive file `{file_id}`");
        let (response, _) = google_request!(&context, {
            hub.files()
                .get(file_id)
                .add_scopes(oauth_scope_urls(highest_active_scope(&session.active_scopes)))
                .supports_all_drives(false)
                .param("alt", "media")
                .doit()
                .await
        })?;
        let body = response.into_body();
        let bytes = common::to_bytes(body)
            .await
            .ok_or_else(|| CoreError::Message("Google Drive download body was empty".into()))?;
        Ok(bytes.to_vec())
    }

    async fn export_file(&self, file_id: &str, mime_type: &str) -> CoreResult<Vec<u8>> {
        let session = self.ensure_scope_internal(DriveScope::DriveReadonly).await?;
        let hub = self.build_hub().await?;
        let context = format!("exporting Google Drive file `{file_id}` as `{mime_type}`");
        let response = google_request!(&context, {
            hub.files()
                .export(file_id, mime_type)
                .add_scopes(oauth_scope_urls(highest_active_scope(&session.active_scopes)))
                .doit()
                .await
        })?;
        let body = response.into_body();
        let bytes = common::to_bytes(body)
            .await
            .ok_or_else(|| CoreError::Message("Google Drive export body was empty".into()))?;
        Ok(bytes.to_vec())
    }

    async fn download_url(&self, url: &str) -> CoreResult<Vec<u8>> {
        let session = self.ensure_scope_internal(DriveScope::DriveReadonly).await?;
        let hub = self.build_hub().await?;
        let context = format!("downloading authenticated Google URL `{url}`");
        let scope_urls = oauth_scope_urls(highest_active_scope(&session.active_scopes));
        let access_token =
            hub.auth.get_token(&scope_urls).await.map_err(map_token_error)?.ok_or_else(|| {
                CoreError::Message("Google OAuth token missing access token".into())
            })?;
        let client = Client::new();
        let parsed_url = reqwest::Url::parse(url)
            .map_err(|error| CoreError::Message(format!("{context} URL was invalid: {error}")))?;
        let request = || client.get(parsed_url.clone()).bearer_auth(access_token.clone());
        execute_reqwest_bytes_request(&context, request).await
    }

    async fn remove_my_drive_parent(&self, file_id: &str) -> CoreResult<RemoteFileMetadata> {
        let session = self.ensure_scope_internal(DriveScope::Drive).await?;
        let hub = self.build_hub().await?;
        let context = format!("removing Google Drive file `{file_id}` from My Drive");
        let scope_urls = oauth_scope_urls(highest_active_scope(&session.active_scopes));
        let access_token =
            hub.auth.get_token(&scope_urls).await.map_err(map_token_error)?.ok_or_else(|| {
                CoreError::Message("Google OAuth token missing access token".into())
            })?;
        let client = Client::new();
        let url = reqwest::Url::parse_with_params(
            &format!("https://www.googleapis.com/drive/v3/files/{file_id}"),
            &[
                ("removeParents", "root"),
                ("supportsAllDrives", "false"),
                ("fields", FILE_ITEM_FIELDS),
            ],
        )
        .map_err(|error| CoreError::Message(format!("{context} URL was invalid: {error}")))?;
        let request = || {
            client
                .patch(url.clone())
                .bearer_auth(access_token.clone())
                .header("content-type", "application/json")
                .body("{}")
        };
        let file = execute_reqwest_json_request::<GoogleFile, _>(&context, request).await?;
        Ok(RemoteFileMetadata::from(map_drive_file(file)?))
    }

    async fn get_account_about(&self) -> CoreResult<AccountAbout> {
        let session = self.ensure_scope_internal(DriveScope::MetadataReadonly).await?;
        let hub = self.build_hub().await?;
        let (_, about) = google_request!("fetching Google Drive account settings", {
            hub.about()
                .get()
                .add_scopes(oauth_scope_urls(highest_active_scope(&session.active_scopes)))
                .param("fields", ACCOUNT_ABOUT_FIELDS)
                .doit()
                .await
        })?;
        account_about_from_about(about)
    }

    async fn get_account_profile(&self) -> CoreResult<AccountProfile> {
        let session = self.ensure_scope_internal(DriveScope::MetadataReadonly).await?;
        let hub = self.build_hub().await?;
        let (_, about) = google_request!("fetching authenticated Google profile", {
            hub.about()
                .get()
                .add_scopes(oauth_scope_urls(highest_active_scope(&session.active_scopes)))
                .param("fields", ABOUT_FIELDS)
                .doit()
                .await
        })?;
        account_from_about(about)
    }
}

pub(super) fn escape_drive_query(value: &str) -> String {
    value.replace('\\', "\\\\").replace('\'', "\\'")
}

async fn execute_google_request<T, F, Fut>(context: &str, mut operation: F) -> CoreResult<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, common::Error>>,
{
    let mut attempt = 1;
    loop {
        match timeout(GOOGLE_REQUEST_TIMEOUT, operation()).await {
            Ok(Ok(value)) => return Ok(value),
            Ok(Err(error))
                if attempt < GOOGLE_REQUEST_MAX_ATTEMPTS && is_retryable_google_error(&error) =>
            {
                sleep(retry_delay(attempt)).await;
                attempt += 1;
            }
            Ok(Err(error)) => return Err(map_google_error(context, error)),
            Err(_) if attempt < GOOGLE_REQUEST_MAX_ATTEMPTS => {
                sleep(retry_delay(attempt)).await;
                attempt += 1;
            }
            Err(_) => {
                return Err(CoreError::Message(format!(
                    "{context} timed out after {} seconds",
                    GOOGLE_REQUEST_TIMEOUT.as_secs()
                )));
            }
        }
    }
}

async fn execute_reqwest_json_request<T, F>(context: &str, mut operation: F) -> CoreResult<T>
where
    T: serde::de::DeserializeOwned,
    F: FnMut() -> reqwest::RequestBuilder,
{
    let mut attempt = 1;
    loop {
        let request = operation();
        match timeout(GOOGLE_REQUEST_TIMEOUT, request.send()).await {
            Ok(Ok(response))
                if attempt < GOOGLE_REQUEST_MAX_ATTEMPTS
                    && is_retryable_reqwest_status(response.status()) =>
            {
                sleep(retry_delay(attempt)).await;
                attempt += 1;
            }
            Ok(Ok(response)) => {
                let status = response.status();
                if !status.is_success() {
                    let body = response.text().await.unwrap_or_default();
                    return Err(CoreError::Message(format!(
                        "{context} failed with HTTP {status}: {body}"
                    )));
                }
                let body = response.text().await.map_err(|error| {
                    CoreError::Message(format!("{context} response could not be read: {error}"))
                })?;
                return serde_json::from_str::<T>(&body).map_err(|error| {
                    CoreError::Message(format!("{context} returned invalid JSON: {error}"))
                });
            }
            Ok(Err(error)) if attempt < GOOGLE_REQUEST_MAX_ATTEMPTS && error.is_timeout() => {
                sleep(retry_delay(attempt)).await;
                attempt += 1;
            }
            Ok(Err(error)) => {
                return Err(CoreError::Message(format!("{context} failed: {error}")));
            }
            Err(_) if attempt < GOOGLE_REQUEST_MAX_ATTEMPTS => {
                sleep(retry_delay(attempt)).await;
                attempt += 1;
            }
            Err(_) => {
                return Err(CoreError::Message(format!(
                    "{context} timed out after {} seconds",
                    GOOGLE_REQUEST_TIMEOUT.as_secs()
                )));
            }
        }
    }
}

async fn execute_reqwest_bytes_request<F>(context: &str, mut operation: F) -> CoreResult<Vec<u8>>
where
    F: FnMut() -> reqwest::RequestBuilder,
{
    let mut attempt = 1;
    loop {
        let request = operation();
        match timeout(GOOGLE_REQUEST_TIMEOUT, request.send()).await {
            Ok(Ok(response))
                if attempt < GOOGLE_REQUEST_MAX_ATTEMPTS
                    && is_retryable_reqwest_status(response.status()) =>
            {
                sleep(retry_delay(attempt)).await;
                attempt += 1;
            }
            Ok(Ok(response)) => {
                let status = response.status();
                if !status.is_success() {
                    let body = response.text().await.unwrap_or_default();
                    return Err(CoreError::Message(format!(
                        "{context} failed with HTTP {status}: {body}"
                    )));
                }
                let bytes = response.bytes().await.map_err(|error| {
                    CoreError::Message(format!("{context} response could not be read: {error}"))
                })?;
                return Ok(bytes.to_vec());
            }
            Ok(Err(error)) if attempt < GOOGLE_REQUEST_MAX_ATTEMPTS && error.is_timeout() => {
                sleep(retry_delay(attempt)).await;
                attempt += 1;
            }
            Ok(Err(error)) => {
                return Err(CoreError::Message(format!("{context} failed: {error}")));
            }
            Err(_) if attempt < GOOGLE_REQUEST_MAX_ATTEMPTS => {
                sleep(retry_delay(attempt)).await;
                attempt += 1;
            }
            Err(_) => {
                return Err(CoreError::Message(format!(
                    "{context} timed out after {} seconds",
                    GOOGLE_REQUEST_TIMEOUT.as_secs()
                )));
            }
        }
    }
}

fn is_retryable_reqwest_status(status: reqwest::StatusCode) -> bool {
    status.as_u16() == 429 || status.as_u16() == 408 || status.is_server_error()
}

pub(super) fn is_retryable_google_error(error: &common::Error) -> bool {
    match error {
        common::Error::Failure(response) => {
            let status = response.status().as_u16();
            status == 429 || status == 408 || (500..=599).contains(&status)
        }
        common::Error::BadRequest(value) => extract_google_error_message(value)
            .map(|message| {
                let lower = message.to_ascii_lowercase();
                lower.contains("rate limit")
                    || lower.contains("ratelimit")
                    || lower.contains("user rate limit")
                    || lower.contains("quota")
            })
            .unwrap_or(false),
        common::Error::Io(_) => true,
        _ => false,
    }
}

pub(super) fn retry_delay(attempt: usize) -> Duration {
    GOOGLE_RETRY_BASE_DELAY * 2_u32.saturating_pow(attempt.saturating_sub(1) as u32)
}

pub(super) fn drive_find_file_query(parent_id: &str, name: &str) -> String {
    format!(
        "'{}' in parents and name = '{}' and trashed = false",
        escape_drive_query(parent_id),
        escape_drive_query(name)
    )
}

pub(super) fn drive_folder_listing_query(parent_id: &str, name_prefix: Option<&str>) -> String {
    let mut query = format!("'{}' in parents and trashed = false", escape_drive_query(parent_id));
    if let Some(prefix) = name_prefix {
        query.push_str(&format!(" and name contains '{}'", escape_drive_query(prefix)));
    }
    query
}

pub(super) fn filter_remote_files_by_prefix(
    files: Vec<RemoteFileMetadata>,
    name_prefix: Option<&str>,
) -> Vec<RemoteFileMetadata> {
    let mut files = files
        .into_iter()
        .filter(|file| name_prefix.is_none_or(|prefix| file.name.starts_with(prefix)))
        .collect::<Vec<_>>();
    files.sort_by(|left, right| left.name.cmp(&right.name).then_with(|| left.id.cmp(&right.id)));
    files
}

pub(super) fn inspect_exif_details_from_record(
    id: &str,
    file: FileRecord,
) -> CoreResult<InspectExifDetails> {
    if !file.mime_type.starts_with("image/") {
        return Err(CoreError::Message(format!(
            "file `{id}` is not an image; inspect exif only supports image/* items"
        )));
    }
    let Some(metadata) = file.image_media_metadata.clone() else {
        return Err(CoreError::Message(format!(
            "file `{id}` has no imageMediaMetadata in Google Drive; EXIF byte-download fallback is not implemented yet"
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

pub(super) fn create_folder_request(parent_id: &str, name: &str) -> GoogleFile {
    GoogleFile {
        name: Some(name.to_string()),
        mime_type: Some(GOOGLE_DRIVE_FOLDER_MIME.into()),
        parents: Some(vec![parent_id.to_string()]),
        ..GoogleFile::default()
    }
}

pub(super) fn copy_file_request(parent_id: &str, name: Option<&str>) -> GoogleFile {
    GoogleFile {
        name: name.map(ToOwned::to_owned),
        parents: Some(vec![parent_id.to_string()]),
        ..GoogleFile::default()
    }
}

pub(super) fn upload_file_request(parent_id: &str, name: &str, mime_type: &str) -> GoogleFile {
    GoogleFile {
        name: Some(name.to_string()),
        mime_type: Some(mime_type.to_string()),
        parents: Some(vec![parent_id.to_string()]),
        ..GoogleFile::default()
    }
}

pub(super) fn update_file_request(name: &str, mime_type: &str) -> GoogleFile {
    GoogleFile {
        name: Some(name.to_string()),
        mime_type: Some(mime_type.to_string()),
        ..GoogleFile::default()
    }
}

pub(super) fn rename_file_request(name: &str) -> GoogleFile {
    GoogleFile { name: Some(name.to_string()), ..GoogleFile::default() }
}

pub(super) fn trash_file_request() -> GoogleFile {
    GoogleFile { trashed: Some(true), ..GoogleFile::default() }
}

fn build_http_client() -> CoreResult<common::Client<impl common::Connector>> {
    if crypto::CryptoProvider::get_default().is_none() {
        let _ = aws_lc_rs::default_provider().install_default();
    }
    let connector = hyper_rustls::HttpsConnectorBuilder::new()
        .with_native_roots()
        .map_err(|error| CoreError::Message(format!("failed to load native TLS roots: {error}")))?
        .https_or_http()
        .enable_http1()
        .enable_http2()
        .build();
    Ok(hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
        .build(connector))
}
