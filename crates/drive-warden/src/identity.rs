//! The multi-account identity guard.
//!
//! Before any Drive write against a selected account, the live Google session
//! (its `permissionId`) is verified against the account's binding. A mismatch
//! is a `SECURITY ALERT` and blocks the operation. Reads warn instead of
//! blocking. The check is fail-closed: if the live identity cannot be fetched,
//! a blocked operation is refused rather than allowed.

use anyhow::{bail, Context, Result};
use gdrive_core::DriveGateway;

use crate::account::{AccountContext, IdentityState};
use crate::AppRuntime;

/// Whether an identity mismatch blocks (writes) or only warns (reads).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityCheckMode {
    Block,
    Warn,
}

/// Result of comparing a bound identity against a live one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityOutcome {
    /// Live identity matches the binding.
    Match,
    /// Live identity contradicts the binding — a security event.
    Mismatch,
    /// No firm binding yet; record the live identity (TOFU / declared-confirm).
    BindNow,
}

/// Pure decision: given the account's binding state and the live identity,
/// decide whether to proceed, bind, or treat as a mismatch.
///
/// - **Bound**: must match on the durable `account_id` (permissionId).
/// - **Declared**: must match on email (no permissionId recorded pre-login);
///   a match binds the permissionId.
/// - **Unbound**: trust on first use — record whatever is observed.
pub fn evaluate_identity(
    state: IdentityState,
    bound_email: Option<&str>,
    bound_account_id: Option<&str>,
    live_email: &str,
    live_account_id: &str,
) -> IdentityOutcome {
    match state {
        IdentityState::Bound => match bound_account_id {
            Some(id) if id == live_account_id => IdentityOutcome::Match,
            Some(_) => IdentityOutcome::Mismatch,
            None => IdentityOutcome::BindNow,
        },
        IdentityState::Declared => match bound_email {
            Some(email) if email.eq_ignore_ascii_case(live_email) => IdentityOutcome::BindNow,
            Some(_) => IdentityOutcome::Mismatch,
            None => IdentityOutcome::BindNow,
        },
        IdentityState::Unbound => IdentityOutcome::BindNow,
    }
}

/// Verify the active Google session matches the selected account. No-op in
/// legacy / escape-hatch mode (no account context).
pub async fn ensure_account_identity(
    gateway: &dyn DriveGateway,
    runtime: &AppRuntime,
    mode: IdentityCheckMode,
) -> Result<()> {
    let Some(account) = runtime.account.as_ref() else {
        return Ok(());
    };

    let profile = match gateway.get_account_profile().await {
        Ok(profile) => profile,
        Err(error) => match mode {
            IdentityCheckMode::Block => {
                return Err(anyhow::Error::msg(error)).context(
                    "identity verification failed; refusing to act on the selected account",
                );
            }
            IdentityCheckMode::Warn => {
                eprintln!("warning: could not verify the active Google account ({error})");
                return Ok(());
            }
        },
    };

    apply_identity_outcome(
        account,
        &profile.email,
        &profile.account_id,
        profile.display_name.as_deref(),
        mode,
    )
}

/// Apply the identity decision for an already-known live identity (e.g. the
/// session returned by `auth login`): proceed on match, bind on first
/// observation, or block/warn on mismatch.
pub fn apply_identity_outcome(
    account: &AccountContext,
    live_email: &str,
    live_account_id: &str,
    live_display_name: Option<&str>,
    mode: IdentityCheckMode,
) -> Result<()> {
    match evaluate_identity(
        account.toml.identity.state,
        account.bound_email(),
        account.bound_account_id(),
        live_email,
        live_account_id,
    ) {
        IdentityOutcome::Match => Ok(()),
        IdentityOutcome::BindNow => {
            let mut updated = account.toml.clone();
            updated.identity.state = IdentityState::Bound;
            updated.identity.email = Some(live_email.to_string());
            updated.identity.account_id = Some(live_account_id.to_string());
            if updated.identity.display_name.is_none() {
                updated.identity.display_name = live_display_name.map(ToString::to_string);
            }
            crate::account::save_account_toml(&account.account_toml_path(), &updated)?;
            Ok(())
        }
        IdentityOutcome::Mismatch => {
            let bound = account.bound_email().unwrap_or("its bound identity");
            match mode {
                IdentityCheckMode::Block => bail!(
                    "SECURITY ALERT: account `{}` is bound to {} but the active Google session is {} ({}). Refusing to proceed.",
                    account.name,
                    bound,
                    live_email,
                    live_account_id
                ),
                IdentityCheckMode::Warn => {
                    eprintln!(
                        "SECURITY ALERT: account `{}` is bound to {} but the active Google session is {}.",
                        account.name, bound, live_email
                    );
                    Ok(())
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bound_matches_on_account_id() {
        assert_eq!(
            evaluate_identity(IdentityState::Bound, Some("a@x"), Some("ID1"), "a@x", "ID1"),
            IdentityOutcome::Match
        );
        assert_eq!(
            evaluate_identity(IdentityState::Bound, Some("a@x"), Some("ID1"), "a@x", "ID2"),
            IdentityOutcome::Mismatch
        );
    }

    #[test]
    fn declared_matches_on_email_then_binds() {
        assert_eq!(
            evaluate_identity(IdentityState::Declared, Some("a@x"), None, "A@X", "ID1"),
            IdentityOutcome::BindNow
        );
        assert_eq!(
            evaluate_identity(IdentityState::Declared, Some("a@x"), None, "b@y", "ID1"),
            IdentityOutcome::Mismatch
        );
    }

    #[test]
    fn unbound_is_tofu() {
        assert_eq!(
            evaluate_identity(IdentityState::Unbound, None, None, "who@x", "ID9"),
            IdentityOutcome::BindNow
        );
    }
}
