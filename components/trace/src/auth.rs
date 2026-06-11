//! OAuth token storage for the `trace` CLI.
//!
//! This module owns the secret-token side of authentication. Profile files keep
//! non-secret connection metadata, while this module stores OAuth2 token records
//! in the operating system keychain.

use std::{
    io::{Read, Write},
    net::TcpListener,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;

use crate::{
    cli::ProfileSelector,
    config::{self, AuthMode, LoadedProfile, Profile},
};

/// Keychain service name used for all trace CLI token records.
const KEYCHAIN_SERVICE: &str = "dpm-trace";
/// macOS Security Framework status for a missing keychain item.
const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;
/// Local callback path registered in the OAuth2 authorisation request.
const CALLBACK_PATH: &str = "/callback";
/// Maximum time to wait for the browser to redirect back to the CLI.
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(120);
/// Delay between non-blocking callback accept attempts.
const CALLBACK_POLL_INTERVAL: Duration = Duration::from_millis(200);
/// Development-only OAuth2 client seeded by Canton Network LocalNet.
///
/// Docs: https://docs.canton.network/appdev/quickstart/json-api#user-token-and-ids-cookbook
const LOCALNET_PROVIDER_CLIENT_ID: &str = "app-provider-validator";
/// Development-only OAuth2 client secret seeded by Canton Network LocalNet.
const LOCALNET_PROVIDER_CLIENT_SECRET: &str = "AL8648b9SfdTFImq7FV56Vd0KHifHBuC";
/// Development-only OAuth2 client seeded by Canton Network LocalNet.
///
/// Docs: https://docs.canton.network/appdev/quickstart/json-api#user-token-and-ids-cookbook
const LOCALNET_USER_CLIENT_ID: &str = "app-user-validator";
/// Development-only OAuth2 client secret seeded by Canton Network LocalNet.
const LOCALNET_USER_CLIENT_SECRET: &str = "6m12QyyGl81d9nABWQXMycZdXho6ejEX";

/// Authenticate according to the selected profile's auth mode.
pub fn login(args: ProfileSelector) -> Result<(), String> {
    let loaded = load_selected_profile(args)?;

    match loaded.profile.auth_mode {
        AuthMode::None => {
            println!(
                "Profile '{}' uses auth_mode = \"none\"; no login is required.",
                loaded.name
            );
            Ok(())
        }
        AuthMode::Localnet => login_localnet(loaded),
        AuthMode::Remote => login_remote(loaded),
    }
}

/// Remove stored tokens for the selected profile from the OS keychain.
pub fn logout(args: ProfileSelector) -> Result<(), String> {
    let loaded = load_selected_profile(args)?;
    if loaded.profile.auth_mode == AuthMode::None {
        println!(
            "Profile '{}' uses auth_mode = \"none\"; no stored tokens need to be removed.",
            loaded.name
        );
        return Ok(());
    }

    let key = TokenKey::for_profile(&loaded.name, &loaded.profile);
    let store = KeychainTokenStore::new();

    store.delete(&key)?;
    println!(
        "Removed stored OAuth2 tokens for profile '{}' from the OS keychain.",
        loaded.name
    );
    Ok(())
}

/// Return the stored Ledger API access token for an authenticated profile.
pub fn access_token(profile_name: &str, profile: &Profile) -> Result<Option<String>, String> {
    if profile.auth_mode == AuthMode::None {
        return Ok(None);
    }

    let key = TokenKey::for_profile(profile_name, profile);
    let store = KeychainTokenStore::new();
    let token = store.get(&key)?.ok_or_else(|| {
        format!(
            "no stored token found for profile '{}'; run `dpm trace login --profile {}` first",
            profile_name, profile_name
        )
    })?;

    Ok(Some(token.access_token))
}

/// Run OAuth2 Authorization Code with PKCE for a remote profile.
///
/// TODO: Test this flow against a real remote OAuth2 issuer and participant.
fn login_remote(loaded: LoadedProfile) -> Result<(), String> {
    let key = TokenKey::for_profile(&loaded.name, &loaded.profile);
    let store = KeychainTokenStore::new();

    let discovery = discover_issuer(&loaded.profile)?;
    let listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|error| format!("failed to start local OAuth callback server: {error}"))?;
    let redirect_uri = format!(
        "http://127.0.0.1:{}{}",
        listener
            .local_addr()
            .map_err(|error| format!("failed to read callback server address: {error}"))?
            .port(),
        CALLBACK_PATH
    );
    let state = random_urlsafe(32);
    let code_verifier = random_urlsafe(32);
    let code_challenge = pkce_challenge(&code_verifier);
    let auth_url = authorisation_url(
        &discovery,
        &loaded.profile,
        &redirect_uri,
        &state,
        &code_challenge,
    )?;

    println!("Opening browser to authenticate profile '{}'.", loaded.name);
    println!("Resolved profile from {}", loaded.path.display());
    println!("If the browser does not open, visit:\n{auth_url}");

    if let Err(error) = webbrowser::open(auth_url.as_str()) {
        println!("Could not open browser automatically: {error}");
    }

    let code = wait_for_callback(listener, &state)?;
    let token = exchange_code(
        &discovery,
        &loaded.profile,
        &redirect_uri,
        &code,
        &code_verifier,
    )?;
    store.put(&key, &token)?;

    println!(
        "Authenticated profile '{}' and stored tokens in the OS keychain.",
        loaded.name
    );
    Ok(())
}

/// Run LocalNet client credentials login for a profile.
fn login_localnet(loaded: LoadedProfile) -> Result<(), String> {
    let key = TokenKey::for_profile(&loaded.name, &loaded.profile);
    let store = KeychainTokenStore::new();
    let discovery = discover_issuer(&loaded.profile)?;
    let localnet_client = localnet_client(&loaded.profile)?;
    let token = exchange_client_credentials(&discovery, &loaded.profile, &localnet_client)?;

    store.put(&key, &token)?;

    println!(
        "Authenticated LocalNet profile '{}' using seeded client '{}'.",
        loaded.name, localnet_client.client_id
    );
    println!("Stored tokens in the OS keychain.");
    Ok(())
}

/// Load the profile selected by an auth command.
fn load_selected_profile(args: ProfileSelector) -> Result<LoadedProfile, String> {
    config::load_profile(args.profile, args.profile_file)
}

/// OAuth2 server metadata discovered from the profile issuer URL.
#[derive(Debug, Deserialize)]
struct IssuerMetadata {
    /// OAuth2 authorisation endpoint.
    authorization_endpoint: String,
    /// OAuth2 token endpoint.
    token_endpoint: String,
}

/// OAuth2 token endpoint response fields used by the CLI.
#[derive(Debug, Deserialize)]
struct TokenResponse {
    /// Bearer access token sent to Ledger API calls.
    access_token: Option<String>,
    /// Optional refresh token.
    refresh_token: Option<String>,
    /// Access token lifetime in seconds.
    expires_in: Option<u64>,
    /// OAuth2 error code when token exchange fails.
    error: Option<String>,
    /// OAuth2 error description when token exchange fails.
    error_description: Option<String>,
}

/// LocalNet seeded client credentials.
struct LocalNetClient {
    /// OAuth2 client id.
    client_id: &'static str,
    /// OAuth2 client secret.
    client_secret: &'static str,
}

/// Discover OAuth2 endpoints from the profile issuer.
///
/// TODO: Test discovery against a real remote OAuth2 issuer.
fn discover_issuer(profile: &Profile) -> Result<IssuerMetadata, String> {
    let issuer = profile.issuer.trim_end_matches('/');
    let discovery_url = format!("{issuer}/.well-known/openid-configuration");
    let response = reqwest::blocking::get(&discovery_url).map_err(|error| {
        format!("failed to fetch issuer metadata from {discovery_url}: {error}")
    })?;

    if !response.status().is_success() {
        return Err(format!(
            "issuer metadata request failed with status {} from {}",
            response.status(),
            discovery_url
        ));
    }

    response
        .json()
        .map_err(|error| format!("failed to parse issuer metadata from {discovery_url}: {error}"))
}

/// Build the OAuth2 authorisation URL for browser login.
///
/// TODO: Test generated remote-login URLs against a real OAuth2 client registration.
fn authorisation_url(
    discovery: &IssuerMetadata,
    profile: &Profile,
    redirect_uri: &str,
    state: &str,
    code_challenge: &str,
) -> Result<Url, String> {
    let mut url = Url::parse(&discovery.authorization_endpoint)
        .map_err(|error| format!("invalid authorization_endpoint from issuer: {error}"))?;

    {
        let mut query = url.query_pairs_mut();
        query.append_pair("response_type", "code");
        query.append_pair("client_id", &profile.client_id);
        query.append_pair("redirect_uri", redirect_uri);
        query.append_pair("state", state);
        query.append_pair("code_challenge", code_challenge);
        query.append_pair("code_challenge_method", "S256");
        if !profile.scopes.is_empty() {
            query.append_pair("scope", &profile.scopes.join(" "));
        }
        if !profile.audience.trim().is_empty() {
            query.append_pair("audience", &profile.audience);
        }
    }

    Ok(url)
}

/// Wait for the browser redirect and return the authorisation code.
///
/// TODO: Test the localhost callback behaviour with a real remote browser login.
fn wait_for_callback(listener: TcpListener, expected_state: &str) -> Result<String, String> {
    listener
        .set_nonblocking(true)
        .map_err(|error| format!("failed to configure OAuth callback timeout: {error}"))?;

    let start = Instant::now();
    let mut stream = loop {
        match listener.accept() {
            Ok((stream, _)) => break stream,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if start.elapsed() >= CALLBACK_TIMEOUT {
                    return Err(format!(
                        "timed out waiting for OAuth callback after {} seconds. \
                         The identity provider may have rejected the client id or redirect URI before redirecting back to the CLI.",
                        CALLBACK_TIMEOUT.as_secs()
                    ));
                }
                thread::sleep(CALLBACK_POLL_INTERVAL);
            }
            Err(error) => return Err(format!("failed while waiting for OAuth callback: {error}")),
        }
    };

    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .map_err(|error| format!("failed to configure callback read timeout: {error}"))?;

    let mut buffer = [0_u8; 8192];
    let bytes_read = stream
        .read(&mut buffer)
        .map_err(|error| format!("failed to read OAuth callback request: {error}"))?;
    let request = String::from_utf8_lossy(&buffer[..bytes_read]);
    let target = callback_target(&request)?;
    let url = Url::parse(&format!("http://127.0.0.1{target}"))
        .map_err(|error| format!("failed to parse OAuth callback URL: {error}"))?;

    let params: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
    let result = if let Some(error) = params.get("error") {
        Err(format!(
            "OAuth issuer returned error '{}': {}",
            error,
            params
                .get("error_description")
                .map(String::as_str)
                .unwrap_or("no description")
        ))
    } else if params.get("state").map(String::as_str) != Some(expected_state) {
        Err("OAuth callback state did not match the login request".to_owned())
    } else {
        params
            .get("code")
            .cloned()
            .ok_or_else(|| "OAuth callback did not contain an authorization code".to_owned())
    };

    write_callback_response(&mut stream, result.is_ok())?;
    result
}

/// Extract the request target from the first HTTP request line.
///
/// TODO: Test callback parsing with real remote OAuth2 redirects.
fn callback_target(request: &str) -> Result<&str, String> {
    let request_line = request
        .lines()
        .next()
        .ok_or_else(|| "OAuth callback request was empty".to_owned())?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let target = parts.next().unwrap_or_default();

    if method != "GET" {
        return Err(format!("OAuth callback used unsupported method {method}"));
    }

    if !target.starts_with(CALLBACK_PATH) {
        return Err(format!("OAuth callback used unexpected path {target}"));
    }

    Ok(target)
}

/// Write a small browser response after the callback is received.
///
/// TODO: Test the browser-facing response during a real remote OAuth2 login.
fn write_callback_response(stream: &mut std::net::TcpStream, success: bool) -> Result<(), String> {
    let body = if success {
        "Authentication complete. You can return to the terminal."
    } else {
        "Authentication failed. You can return to the terminal."
    };
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    stream
        .write_all(response.as_bytes())
        .map_err(|error| format!("failed to write OAuth callback response: {error}"))
}

/// Exchange an authorisation code for OAuth2 tokens.
///
/// TODO: Test token exchange against a real remote OAuth2 issuer.
fn exchange_code(
    discovery: &IssuerMetadata,
    profile: &Profile,
    redirect_uri: &str,
    code: &str,
    code_verifier: &str,
) -> Result<StoredToken, String> {
    let client = reqwest::blocking::Client::new();
    let mut form = vec![
        ("grant_type", "authorization_code".to_owned()),
        ("client_id", profile.client_id.clone()),
        ("code", code.to_owned()),
        ("redirect_uri", redirect_uri.to_owned()),
        ("code_verifier", code_verifier.to_owned()),
    ];
    if !profile.audience.trim().is_empty() {
        form.push(("audience", profile.audience.clone()));
    }

    let response = client
        .post(&discovery.token_endpoint)
        .form(&form)
        .send()
        .map_err(|error| format!("failed to exchange OAuth code for tokens: {error}"))?;
    let status = response.status();
    let token_response: TokenResponse = response
        .json()
        .map_err(|error| format!("failed to parse OAuth token response: {error}"))?;

    if !status.is_success() {
        return Err(format!(
            "OAuth token exchange failed with status {status}: {}",
            oauth_error_message(&token_response)
        ));
    }

    let access_token = token_response
        .access_token
        .ok_or_else(|| "OAuth token response did not include an access_token".to_owned())?;

    Ok(StoredToken {
        access_token,
        refresh_token: token_response.refresh_token,
        expires_at_epoch_seconds: token_response
            .expires_in
            .map(|expires_in| unix_time_now() + expires_in),
    })
}

/// Exchange LocalNet client credentials for OAuth2 tokens.
fn exchange_client_credentials(
    discovery: &IssuerMetadata,
    profile: &Profile,
    localnet_client: &LocalNetClient,
) -> Result<StoredToken, String> {
    let client = reqwest::blocking::Client::new();
    let mut form = vec![
        ("grant_type", "client_credentials".to_owned()),
        ("client_id", localnet_client.client_id.to_owned()),
        ("client_secret", localnet_client.client_secret.to_owned()),
    ];

    if !profile.scopes.is_empty() {
        form.push(("scope", profile.scopes.join(" ")));
    }

    let response = client
        .post(&discovery.token_endpoint)
        .form(&form)
        .send()
        .map_err(|error| format!("failed to request LocalNet OAuth token: {error}"))?;
    let status = response.status();
    let token_response: TokenResponse = response
        .json()
        .map_err(|error| format!("failed to parse LocalNet OAuth token response: {error}"))?;

    if !status.is_success() {
        return Err(format!(
            "LocalNet OAuth token request failed with status {status}: {}",
            oauth_error_message(&token_response)
        ));
    }

    let access_token = token_response.access_token.ok_or_else(|| {
        "LocalNet OAuth token response did not include an access_token".to_owned()
    })?;

    Ok(StoredToken {
        access_token,
        refresh_token: token_response.refresh_token,
        expires_at_epoch_seconds: token_response
            .expires_in
            .map(|expires_in| unix_time_now() + expires_in),
    })
}

/// Select the seeded LocalNet client for the profile.
fn localnet_client(profile: &Profile) -> Result<LocalNetClient, String> {
    let issuer = profile.issuer.trim_end_matches('/');

    if profile.client_id == LOCALNET_PROVIDER_CLIENT_ID || issuer.ends_with("/AppProvider") {
        return Ok(LocalNetClient {
            client_id: LOCALNET_PROVIDER_CLIENT_ID,
            client_secret: LOCALNET_PROVIDER_CLIENT_SECRET,
        });
    }

    if profile.client_id == LOCALNET_USER_CLIENT_ID || issuer.ends_with("/AppUser") {
        return Ok(LocalNetClient {
            client_id: LOCALNET_USER_CLIENT_ID,
            client_secret: LOCALNET_USER_CLIENT_SECRET,
        });
    }

    Err(format!(
        "auth_mode = \"localnet\" requires issuer realm AppProvider/AppUser or client_id '{}'/'{}'",
        LOCALNET_PROVIDER_CLIENT_ID, LOCALNET_USER_CLIENT_ID
    ))
}

/// Render an OAuth2 error response.
fn oauth_error_message(response: &TokenResponse) -> String {
    match (&response.error, &response.error_description) {
        (Some(error), Some(description)) => format!("{error}: {description}"),
        (Some(error), None) => error.clone(),
        _ => "no error description".to_owned(),
    }
}

/// Generate a random base64url value.
fn random_urlsafe(bytes: usize) -> String {
    let random: Vec<u8> = (0..bytes).map(|_| rand::random::<u8>()).collect();
    URL_SAFE_NO_PAD.encode(random)
}

/// Compute an S256 PKCE challenge from a code verifier.
fn pkce_challenge(code_verifier: &str) -> String {
    let digest = Sha256::digest(code_verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(digest)
}

/// Current Unix time in seconds.
fn unix_time_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
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
