//! Multi-account support: named account directories, identity binding metadata,
//! the current-account pointer, and selection precedence.
//!
//! An account is a directory under the accounts root (default `data/accounts`)
//! holding its own `inventory.db`, tokens, session, reports, and an
//! `account.toml` describing the bound Google identity and any overrides.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

/// Default accounts root, relative to the working directory.
pub const DEFAULT_ACCOUNTS_ROOT: &str = "data/accounts";
/// File under the accounts root recording the current account name.
const CURRENT_POINTER_FILE: &str = ".current";
/// Per-account metadata file; also the adoption completion sentinel.
pub const ACCOUNT_TOML_FILE: &str = "account.toml";

/// Lifecycle of an account's identity binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IdentityState {
    /// Adopted without a declared email; binds on first observation (TOFU).
    Unbound,
    /// Email declared via `--email`; binds permissionId on first matching login.
    Declared,
    /// Fully bound to a Google permissionId.
    Bound,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountIdentity {
    pub state: IdentityState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AccountRemoteOverrides {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub db_name: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AccountOverrides {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credentials_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reports_dir: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountToml {
    pub schema_version: u32,
    pub identity: AccountIdentity,
    #[serde(default)]
    pub remote: AccountRemoteOverrides,
    #[serde(default)]
    pub overrides: AccountOverrides,
}

impl AccountToml {
    /// A fresh binding from an optional declared email.
    pub fn new(email: Option<String>) -> Self {
        let state = if email.is_some() { IdentityState::Declared } else { IdentityState::Unbound };
        AccountToml {
            schema_version: 1,
            identity: AccountIdentity { state, email, account_id: None, display_name: None },
            remote: AccountRemoteOverrides::default(),
            overrides: AccountOverrides::default(),
        }
    }
}

/// Validate an account name used as a directory name.
pub fn validate_account_name(name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("account name must not be empty");
    }
    if name == CURRENT_POINTER_FILE || name == "." || name == ".." {
        bail!("`{name}` is a reserved account name");
    }
    if !name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_') {
        bail!("account name `{name}` may only contain lowercase letters, digits, `-`, and `_`");
    }
    Ok(())
}

/// Path to an account's `account.toml`.
pub fn account_toml_path(account_dir: &Path) -> PathBuf {
    account_dir.join(ACCOUNT_TOML_FILE)
}

pub fn load_account_toml(path: &Path) -> Result<AccountToml> {
    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read account metadata `{}`", path.display()))?;
    toml::from_str(&contents)
        .with_context(|| format!("failed to parse account metadata `{}`", path.display()))
}

/// Atomically write `account.toml` (temp file + rename) so a crash never leaves
/// a half-written metadata file.
pub fn save_account_toml(path: &Path, account: &AccountToml) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create `{}`", parent.display()))?;
    }
    let contents =
        toml::to_string_pretty(account).context("failed to serialize account metadata")?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, contents)
        .with_context(|| format!("failed to write `{}`", tmp.display()))?;
    std::fs::rename(&tmp, path)
        .with_context(|| format!("failed to install account metadata `{}`", path.display()))?;
    Ok(())
}

fn current_pointer_path(accounts_root: &Path) -> PathBuf {
    accounts_root.join(CURRENT_POINTER_FILE)
}

/// Read the current-account name, if a pointer exists.
pub fn read_current(accounts_root: &Path) -> Result<Option<String>> {
    let path = current_pointer_path(accounts_root);
    match std::fs::read_to_string(&path) {
        Ok(contents) => {
            let name = contents.trim().to_string();
            Ok(if name.is_empty() { None } else { Some(name) })
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("failed to read `{}`", path.display())),
    }
}

/// Set the current-account pointer.
pub fn write_current(accounts_root: &Path, name: &str) -> Result<()> {
    std::fs::create_dir_all(accounts_root)
        .with_context(|| format!("failed to create accounts root `{}`", accounts_root.display()))?;
    let path = current_pointer_path(accounts_root);
    std::fs::write(&path, format!("{name}\n"))
        .with_context(|| format!("failed to write current-account pointer `{}`", path.display()))
}

/// Names of accounts that exist under the accounts root (dirs with an
/// `account.toml`), sorted.
pub fn list_account_names(accounts_root: &Path) -> Result<Vec<String>> {
    let mut names = Vec::new();
    let entries = match std::fs::read_dir(accounts_root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(names),
        Err(error) => {
            return Err(error).with_context(|| {
                format!("failed to read accounts root `{}`", accounts_root.display())
            })
        }
    };
    for entry in entries {
        let entry = entry?;
        if entry.file_type()?.is_dir() && account_toml_path(&entry.path()).exists() {
            if let Some(name) = entry.file_name().to_str() {
                names.push(name.to_string());
            }
        }
    }
    names.sort();
    Ok(names)
}

/// Resolve the accounts root: env override, then config value, then default.
pub fn accounts_root_from(env: Option<String>, config: Option<&str>) -> PathBuf {
    env.map(PathBuf::from)
        .or_else(|| config.map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from(DEFAULT_ACCOUNTS_ROOT))
}

/// Pick the selected account name from the available signals (flag wins, then
/// env, then the saved current pointer).
pub fn resolve_account_selection(
    flag: Option<&str>,
    env: Option<&str>,
    current: Option<&str>,
) -> Option<String> {
    flag.or(env).or(current).map(ToOwned::to_owned)
}

/// Outcome of account resolution for an invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountResolution {
    /// Legacy single-db mode (explicit `--db`, or a fresh install with no accounts).
    Legacy,
    /// Operate against the named account.
    Named(String),
}

/// Decide which account (if any) a command targets. Pure for testability.
///
/// `--db` is the escape hatch and is mutually exclusive with `--account`.
/// Without `--db`, selection is flag → env → current. If nothing is selected
/// and accounts already exist, a command that needs an account errors loudly
/// rather than silently using the legacy database.
pub fn resolve_account(
    db_flag_present: bool,
    account_flag: Option<&str>,
    env_account: Option<&str>,
    current: Option<&str>,
    accounts_exist: bool,
    command_needs_account: bool,
) -> Result<AccountResolution> {
    if db_flag_present {
        if account_flag.is_some() {
            bail!("`--account` and `--db` are mutually exclusive; pick one");
        }
        return Ok(AccountResolution::Legacy);
    }
    if let Some(name) = resolve_account_selection(account_flag, env_account, current) {
        validate_account_name(&name)?;
        return Ok(AccountResolution::Named(name));
    }
    if accounts_exist && command_needs_account {
        bail!(
            "no account selected: set one with `drive-warden account use <name>` or pass `--account <name>`"
        );
    }
    Ok(AccountResolution::Legacy)
}

/// A resolved account directory plus its loaded metadata.
#[derive(Debug, Clone)]
pub struct AccountContext {
    pub name: String,
    pub dir: PathBuf,
    pub toml: AccountToml,
}

impl AccountContext {
    /// Load the named account under `accounts_root`.
    pub fn load(accounts_root: &Path, name: &str) -> Result<Self> {
        validate_account_name(name)?;
        let dir = accounts_root.join(name);
        let toml_path = account_toml_path(&dir);
        if !toml_path.exists() {
            bail!(
                "account `{name}` was not found under `{}`; create it with `drive-warden account add {name}`",
                accounts_root.display()
            );
        }
        let toml = load_account_toml(&toml_path)?;
        Ok(AccountContext { name: name.to_string(), dir, toml })
    }

    pub fn db_path(&self) -> PathBuf {
        self.dir.join("inventory.db")
    }

    pub fn reports_dir(&self) -> PathBuf {
        match &self.toml.overrides.reports_dir {
            Some(dir) => PathBuf::from(dir),
            None => self.dir.join("reports"),
        }
    }

    pub fn account_toml_path(&self) -> PathBuf {
        account_toml_path(&self.dir)
    }

    /// The bound email or, before binding, the declared email.
    pub fn bound_email(&self) -> Option<&str> {
        self.toml.identity.email.as_deref()
    }

    pub fn bound_account_id(&self) -> Option<&str> {
        self.toml.identity.account_id.as_deref()
    }
}

/// Source files for adopting an existing install into an account.
#[derive(Debug, Clone)]
pub struct AdoptionSources {
    pub db: PathBuf,
    pub tokens: Option<PathBuf>,
    pub session: Option<PathBuf>,
    pub reports_dir: Option<PathBuf>,
}

/// Default legacy layout: `data/inventory.db` (+ sidecars), tokens/session in
/// `data/`, and the top-level `reports/` dir. The accounts root's parent is the
/// legacy data dir; its grandparent is the workspace holding `reports/`.
pub fn legacy_adoption_sources(accounts_root: &Path) -> AdoptionSources {
    let data_dir =
        accounts_root.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("data"));
    let workspace = data_dir.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."));
    AdoptionSources {
        db: data_dir.join("inventory.db"),
        tokens: Some(data_dir.join("google-tokens.json")),
        session: Some(data_dir.join("google-session.json")),
        reports_dir: Some(workspace.join("reports")),
    }
}

/// The default legacy database path if it still exists (un-adopted).
pub fn legacy_db_present(accounts_root: &Path) -> Option<PathBuf> {
    let db = legacy_adoption_sources(accounts_root).db;
    db.exists().then_some(db)
}

/// Whether a named account already exists (has an `account.toml`).
pub fn account_exists(accounts_root: &Path, name: &str) -> bool {
    account_toml_path(&accounts_root.join(name)).exists()
}

fn move_if_exists(src: &Path, dst: &Path, moved: &mut Vec<String>) -> Result<()> {
    if !src.exists() {
        return Ok(());
    }
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create `{}`", parent.display()))?;
    }
    std::fs::rename(src, dst)
        .with_context(|| format!("failed to move `{}` to `{}`", src.display(), dst.display()))?;
    moved.push(dst.display().to_string());
    Ok(())
}

/// Move the database plus its sidecars (`-wal`, `-shm`, `.before-*` snapshots)
/// into `target_dir`, canonicalizing the base name to `inventory.db`.
fn move_db_with_sidecars(db: &Path, target_dir: &Path, moved: &mut Vec<String>) -> Result<()> {
    let src_name =
        db.file_name().and_then(|n| n.to_str()).context("invalid database file name")?.to_string();
    let parent = db.parent().unwrap_or_else(|| Path::new("."));
    let mut siblings: Vec<(PathBuf, String)> = Vec::new();
    for entry in std::fs::read_dir(parent)
        .with_context(|| format!("failed to read `{}`", parent.display()))?
    {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        if name == src_name
            || name.starts_with(&format!("{src_name}-"))
            || name.starts_with(&format!("{src_name}."))
        {
            siblings.push((entry.path(), name));
        }
    }
    for (path, name) in siblings {
        let suffix = &name[src_name.len()..];
        move_if_exists(&path, &target_dir.join(format!("inventory.db{suffix}")), moved)?;
    }
    Ok(())
}

/// Adopt an existing install into a new account, moving files in and writing
/// `account.toml` last as a completion sentinel. Re-running after a partial
/// move resumes (moves whatever remains, then writes the sentinel).
pub fn adopt_into_account(
    accounts_root: &Path,
    name: &str,
    sources: &AdoptionSources,
    email: Option<String>,
) -> Result<Vec<String>> {
    validate_account_name(name)?;
    let target = accounts_root.join(name);
    let toml_path = account_toml_path(&target);
    if toml_path.exists() {
        bail!("account `{name}` already exists at `{}`", target.display());
    }
    std::fs::create_dir_all(&target)
        .with_context(|| format!("failed to create account dir `{}`", target.display()))?;
    let mut moved = Vec::new();
    if sources.db.exists() {
        move_db_with_sidecars(&sources.db, &target, &mut moved)?;
    } else if !target.join("inventory.db").exists() {
        bail!("database to adopt was not found at `{}`", sources.db.display());
    }
    if let Some(tokens) = &sources.tokens {
        move_if_exists(tokens, &target.join("google-tokens.json"), &mut moved)?;
    }
    if let Some(session) = &sources.session {
        move_if_exists(session, &target.join("google-session.json"), &mut moved)?;
    }
    if let Some(reports) = &sources.reports_dir {
        move_if_exists(reports, &target.join("reports"), &mut moved)?;
    }
    save_account_toml(&toml_path, &AccountToml::new(email))?;
    Ok(moved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_account_names() {
        assert!(validate_account_name("work").is_ok());
        assert!(validate_account_name("personal-2").is_ok());
        assert!(validate_account_name("work_drive").is_ok());
        assert!(validate_account_name("bad/name").is_err());
        assert!(validate_account_name("..").is_err());
        assert!(validate_account_name(".current").is_err());
        assert!(validate_account_name("UPPER").is_err());
        assert!(validate_account_name("has space").is_err());
        assert!(validate_account_name("").is_err());
    }

    #[test]
    fn account_toml_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let path = account_toml_path(dir.path());
        let account = AccountToml::new(Some("me@x.com".into()));
        save_account_toml(&path, &account).unwrap();
        let loaded = load_account_toml(&path).unwrap();
        assert_eq!(loaded.identity.state, IdentityState::Declared);
        assert_eq!(loaded.identity.email.as_deref(), Some("me@x.com"));
        assert!(loaded.identity.account_id.is_none());
    }

    #[test]
    fn current_pointer_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        assert_eq!(read_current(root).unwrap(), None);
        write_current(root, "work").unwrap();
        assert_eq!(read_current(root).unwrap().as_deref(), Some("work"));
    }

    #[test]
    fn lists_only_dirs_with_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        assert!(list_account_names(root).unwrap().is_empty());
        save_account_toml(&account_toml_path(&root.join("work")), &AccountToml::new(None)).unwrap();
        save_account_toml(&account_toml_path(&root.join("personal")), &AccountToml::new(None))
            .unwrap();
        std::fs::create_dir_all(root.join("not-an-account")).unwrap();
        assert_eq!(list_account_names(root).unwrap(), vec!["personal", "work"]);
    }

    #[test]
    fn adopts_legacy_layout() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path();
        let data = workspace.join("data");
        let accounts_root = data.join("accounts");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("inventory.db"), b"db").unwrap();
        std::fs::write(data.join("inventory.db-wal"), b"wal").unwrap();
        std::fs::write(data.join("inventory.db.before-x"), b"snap").unwrap();
        std::fs::write(data.join("google-tokens.json"), b"tok").unwrap();
        std::fs::write(data.join("credentials.json"), b"cred").unwrap();
        std::fs::create_dir_all(workspace.join("reports")).unwrap();
        std::fs::write(workspace.join("reports/a.md"), b"r").unwrap();

        let sources = legacy_adoption_sources(&accounts_root);
        let moved =
            adopt_into_account(&accounts_root, "personal", &sources, Some("me@x.com".into()))
                .unwrap();
        assert!(!moved.is_empty());
        let acct = accounts_root.join("personal");
        assert!(acct.join("inventory.db").exists());
        assert!(acct.join("inventory.db-wal").exists());
        assert!(acct.join("inventory.db.before-x").exists());
        assert!(acct.join("google-tokens.json").exists());
        assert!(acct.join("reports/a.md").exists());
        assert!(acct.join("account.toml").exists());
        // credentials stay shared at the data root; original db moved away
        assert!(data.join("credentials.json").exists());
        assert!(!data.join("inventory.db").exists());
    }

    #[test]
    fn accounts_root_precedence() {
        assert_eq!(accounts_root_from(Some("/e".into()), Some("/c")), PathBuf::from("/e"));
        assert_eq!(accounts_root_from(None, Some("/c")), PathBuf::from("/c"));
        assert_eq!(accounts_root_from(None, None), PathBuf::from(DEFAULT_ACCOUNTS_ROOT));
    }

    #[test]
    fn account_selection_precedence() {
        assert_eq!(
            resolve_account_selection(Some("work"), Some("p"), Some("c")),
            Some("work".into())
        );
        assert_eq!(resolve_account_selection(None, Some("env"), Some("c")), Some("env".into()));
        assert_eq!(resolve_account_selection(None, None, Some("c")), Some("c".into()));
        assert_eq!(resolve_account_selection(None, None, None), None);
    }

    #[test]
    fn resolve_account_rules() {
        // --db escape hatch -> legacy
        assert_eq!(
            resolve_account(true, None, None, None, true, true).unwrap(),
            AccountResolution::Legacy
        );
        // --db + --account -> error
        assert!(resolve_account(true, Some("work"), None, None, false, true).is_err());
        // flag selects
        assert_eq!(
            resolve_account(false, Some("work"), None, None, true, true).unwrap(),
            AccountResolution::Named("work".into())
        );
        // nothing selected, no accounts -> legacy (fresh install)
        assert_eq!(
            resolve_account(false, None, None, None, false, true).unwrap(),
            AccountResolution::Legacy
        );
        // nothing selected, accounts exist, command needs account -> error
        assert!(resolve_account(false, None, None, None, true, true).is_err());
        // nothing selected, accounts exist, command does NOT need account -> legacy/ok
        assert_eq!(
            resolve_account(false, None, None, None, true, false).unwrap(),
            AccountResolution::Legacy
        );
    }
}
