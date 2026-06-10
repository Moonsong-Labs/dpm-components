//! OAuth token storage for the `trace` CLI.
//!
//! This module owns the secret-token side of authentication. Profile files keep
//! non-secret connection metadata, while this module stores OAuth2 token records
//! in the operating system keychain.

use serde::{Deserialize, Serialize};

use crate::{
    cli::ProfileSelector,
    config::{self, LoadedProfile, Profile},
};

/// Keychain service name used for all trace CLI token records.
const KEYCHAIN_SERVICE: &str = "dpm-trace";
/// macOS Security Framework status for a missing keychain item.
const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;

/// Load the selected profile and report the token key that login will use.
///
/// The OAuth2 browser flow is intentionally still a later milestone; this command
/// now proves that profile resolution and token-key derivation are wired together.
pub fn login(args: ProfileSelector) -> Result<(), String> {
    let loaded = load_selected_profile(args)?;
    let key = TokenKey::for_profile(&loaded.name, &loaded.profile);

    println!(
        "OAuth2 login for profile '{}' is not implemented yet.",
        loaded.name
    );
    println!("Resolved profile from {}", loaded.path.display());
    println!(
        "Tokens will be stored in the OS keychain account '{}'.",
        key.account
    );
    Ok(())
}

/// Remove stored tokens for the selected profile from the OS keychain.
pub fn logout(args: ProfileSelector) -> Result<(), String> {
    let loaded = load_selected_profile(args)?;
    let key = TokenKey::for_profile(&loaded.name, &loaded.profile);
    let store = KeychainTokenStore::new();

    store.delete(&key)?;
    println!(
        "Removed stored OAuth2 tokens for profile '{}' from the OS keychain.",
        loaded.name
    );
    Ok(())
}

/// Load the profile selected by an auth command.
fn load_selected_profile(args: ProfileSelector) -> Result<LoadedProfile, String> {
    config::load_profile(args.profile, args.profile_file)
}

/// OAuth2 token data stored in the OS keychain.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct StoredToken {
    /// Bearer access token sent to Ledger API calls.
    pub access_token: String,
    /// Optional refresh token used to obtain a new access token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    /// Optional expiry time as seconds since the Unix epoch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at_epoch_seconds: Option<u64>,
}

/// Stable key used to locate a token record in the keychain.
#[derive(Debug)]
pub struct TokenKey {
    /// Keychain service namespace.
    pub service: &'static str,
    /// Keychain account name.
    pub account: String,
}

impl TokenKey {
    /// Build the token key for a named profile.
    pub fn for_profile(profile_name: &str, profile: &Profile) -> Self {
        Self {
            service: KEYCHAIN_SERVICE,
            account: format!("{}:{}:{}", profile_name, profile.issuer, profile.client_id),
        }
    }
}

/// Storage operations needed by login, logout, and future Ledger API commands.
pub trait TokenStore {
    /// Return the stored token, or `None` when no token exists for the key.
    fn get(&self, key: &TokenKey) -> Result<Option<StoredToken>, String>;

    /// Store or replace the token for the key.
    fn put(&self, key: &TokenKey, token: &StoredToken) -> Result<(), String>;

    /// Delete the token for the key.
    fn delete(&self, key: &TokenKey) -> Result<(), String>;
}

/// Token store backed by the operating system keychain.
pub struct KeychainTokenStore;

impl KeychainTokenStore {
    /// Create a token store that uses the operating system keychain.
    pub fn new() -> Self {
        Self
    }
}

impl TokenStore for KeychainTokenStore {
    fn get(&self, key: &TokenKey) -> Result<Option<StoredToken>, String> {
        get_token(key)
    }

    fn put(&self, key: &TokenKey, token: &StoredToken) -> Result<(), String> {
        put_token(key, token)
    }

    fn delete(&self, key: &TokenKey) -> Result<(), String> {
        delete_token(key)
    }
}

/// Read a token from the platform keychain.
#[cfg(target_os = "macos")]
fn get_token(key: &TokenKey) -> Result<Option<StoredToken>, String> {
    use security_framework::passwords::get_generic_password;

    match get_generic_password(key.service, &key.account) {
        Ok(secret) => {
            let secret = String::from_utf8(secret).map_err(|error| {
                format!(
                    "token stored for account '{}' is not valid UTF-8: {error}",
                    key.account
                )
            })?;
            let token = serde_json::from_str(&secret).map_err(|error| {
                format!(
                    "failed to decode token stored for account '{}': {error}",
                    key.account
                )
            })?;
            Ok(Some(token))
        }
        Err(error) if error.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(None),
        Err(error) => Err(format!(
            "failed to read token for account '{}': {error}",
            key.account
        )),
    }
}

/// Store a token in the platform keychain.
#[cfg(target_os = "macos")]
fn put_token(key: &TokenKey, token: &StoredToken) -> Result<(), String> {
    use security_framework::passwords::set_generic_password;

    let secret = serde_json::to_string(token)
        .map_err(|error| format!("failed to encode token for keychain storage: {error}"))?;
    set_generic_password(key.service, &key.account, secret.as_bytes()).map_err(|error| {
        format!(
            "failed to store token for account '{}': {error}",
            key.account
        )
    })
}

/// Delete a token from the platform keychain.
#[cfg(target_os = "macos")]
fn delete_token(key: &TokenKey) -> Result<(), String> {
    use security_framework::passwords::delete_generic_password;

    match delete_generic_password(key.service, &key.account) {
        Ok(()) => Ok(()),
        Err(error) if error.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(()),
        Err(error) => Err(format!(
            "failed to delete token for account '{}': {error}",
            key.account
        )),
    }
}

/// Return an unsupported-platform error for keychain reads.
#[cfg(not(target_os = "macos"))]
fn get_token(key: &TokenKey) -> Result<Option<StoredToken>, String> {
    Err(unsupported_keychain_error(key))
}

/// Return an unsupported-platform error for keychain writes.
#[cfg(not(target_os = "macos"))]
fn put_token(key: &TokenKey, _token: &StoredToken) -> Result<(), String> {
    Err(unsupported_keychain_error(key))
}

/// Return an unsupported-platform error for keychain deletes.
#[cfg(not(target_os = "macos"))]
fn delete_token(key: &TokenKey) -> Result<(), String> {
    Err(unsupported_keychain_error(key))
}

/// Build the current placeholder error for non-macOS token storage.
#[cfg(not(target_os = "macos"))]
fn unsupported_keychain_error(key: &TokenKey) -> String {
    format!(
        "OS keychain token storage is not implemented for this platform yet; account '{}'",
        key.account
    )
}
