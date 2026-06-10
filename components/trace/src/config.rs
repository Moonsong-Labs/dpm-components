//! Profile configuration for the `trace` CLI.
//!
//! This module owns the non-secret TOML profile data used to connect to a
//! participant Ledger API and discover OAuth2 settings. It deliberately does not
//! store access tokens or refresh tokens; those will live in the OS keychain.
//!
//! The public functions are command handlers called from `main.rs`. The private
//! helpers below them handle interactive prompts, profile-file path precedence,
//! and TOML read/write behaviour.

use std::{
    collections::BTreeMap,
    env, fs,
    io::{self, Write},
    path::PathBuf,
};

use serde::{Deserialize, Serialize};

use crate::cli::{ProfileAddArgs, ProfileFileArgs, ProfileShowArgs};

/// Top-level representation of a trace profile TOML file.
///
/// Profiles are stored under a `profiles` table keyed by profile name.
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct ProfilesFile {
    /// Named connection profiles available to the CLI.
    #[serde(default)]
    pub profiles: BTreeMap<String, Profile>,
}

/// Non-secret connection and OAuth2 metadata for one trace profile.
#[derive(Debug, Deserialize, Serialize)]
pub struct Profile {
    /// Ledger API endpoint, including host and port.
    pub ledger: String,
    /// Whether the Ledger API connection should use TLS.
    pub tls: bool,
    /// OAuth2 issuer URL used for discovery and login.
    pub issuer: String,
    /// OAuth2 client id registered for this CLI.
    pub client_id: String,
    /// Expected OAuth2 audience for Ledger API access tokens.
    pub audience: String,
    /// OAuth2 scopes requested during login.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scopes: Vec<String>,
    /// Default parties to use when tracing ledger updates.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub party: Vec<String>,
}

/// Add or update a profile in the selected profile file.
///
/// Values passed on the command line are used directly. Missing values are
/// collected interactively before the profile is written.
pub fn add_profile(args: ProfileAddArgs) -> Result<(), String> {
    let path = write_profile_path(args.profile_file.clone(), args.global)?;
    let mut profiles = read_profiles_if_exists(&path)?;
    let profile = prompt_for_profile(&args)?;

    profiles.profiles.insert(args.name.clone(), profile);
    write_profiles(&path, &profiles)?;

    println!("Saved profile '{}' to {}", args.name, path.display());
    println!(
        "Run `dpm trace login --profile {}` when you are ready to authenticate.",
        args.name
    );
    Ok(())
}

/// Print the profile names available in the selected profile file.
pub fn list_profiles(args: ProfileFileArgs) -> Result<(), String> {
    let path = read_profile_path(args.profile_file)?;
    let profiles = read_profiles_if_exists(&path)?;

    if profiles.profiles.is_empty() {
        println!("No trace profiles found in {}", path.display());
        return Ok(());
    }

    println!("Profiles in {}:", path.display());
    for name in profiles.profiles.keys() {
        println!("- {name}");
    }

    Ok(())
}

/// Print a single profile as TOML.
///
/// The output intentionally contains only the non-secret profile metadata, not
/// any OAuth2 tokens.
pub fn show_profile(args: ProfileShowArgs) -> Result<(), String> {
    let path = read_profile_path(args.profile_file)?;
    let profiles = read_profiles_if_exists(&path)?;
    let profile = profiles.profiles.get(&args.name).ok_or_else(|| {
        format!(
            "profile '{}' was not found in {}",
            args.name,
            path.display()
        )
    })?;

    println!(
        "{}",
        toml::to_string_pretty(profile).map_err(|error| error.to_string())?
    );
    Ok(())
}

/// Build a complete profile from CLI arguments and interactive prompts.
fn prompt_for_profile(args: &ProfileAddArgs) -> Result<Profile, String> {
    let ledger = required_field(
        args.ledger.clone(),
        "Ledger API address",
        Some("localhost:6865"),
        "This is the participant Ledger API endpoint the trace command will query.",
    )?;
    let tls = prompt_tls(args.tls, args.plaintext)?;
    let issuer = required_field(
        args.issuer.clone(),
        "OAuth2 issuer URL",
        None,
        "This is the identity provider that will issue Ledger API access tokens.",
    )?;
    let client_id = required_field(
        args.client_id.clone(),
        "OAuth2 client id",
        Some("dpm-trace"),
        "This identifies the CLI application registered with the OAuth2 issuer.",
    )?;
    let audience = required_field(
        args.audience.clone(),
        "OAuth2 audience",
        Some("https://canton.network.global"),
        "This is the token audience expected by the participant Ledger API.",
    )?;
    let scopes = if args.scopes.is_empty() {
        prompt_list(
            "OAuth2 scopes, comma separated",
            Some("openid"),
            "These scopes are requested when the CLI opens the OAuth2 login flow.",
        )?
    } else {
        args.scopes.clone()
    };
    let party = if args.parties.is_empty() {
        prompt_list(
            "Default parties, comma separated",
            None,
            "These parties scope the visible ledger events returned by trace commands.",
        )?
    } else {
        args.parties.clone()
    };

    Ok(Profile {
        ledger,
        tls,
        issuer,
        client_id,
        audience,
        scopes,
        party,
    })
}

/// Return an existing non-empty value or prompt until the user provides one.
fn required_field(
    value: Option<String>,
    label: &str,
    default: Option<&str>,
    explanation: &str,
) -> Result<String, String> {
    if let Some(value) = value.filter(|value| !value.trim().is_empty()) {
        return Ok(value);
    }

    loop {
        let answer = prompt(label, default, Some(explanation))?;
        if !answer.is_empty() {
            return Ok(answer);
        }

        println!("{label} is required.");
    }
}

/// Resolve the TLS setting from flags or an interactive yes/no prompt.
fn prompt_tls(tls: bool, plaintext: bool) -> Result<bool, String> {
    if tls {
        return Ok(true);
    }

    if plaintext {
        return Ok(false);
    }

    println!("This controls whether the Ledger API connection uses transport security.");
    loop {
        let answer = prompt("Use TLS?", Some("n"), None)?;
        match answer.to_ascii_lowercase().as_str() {
            "" | "n" | "no" => return Ok(false),
            "y" | "yes" => return Ok(true),
            _ => println!("Please answer yes or no."),
        }
    }
}

/// Prompt for a comma-separated list and trim empty entries.
fn prompt_list(
    label: &str,
    default: Option<&str>,
    explanation: &str,
) -> Result<Vec<String>, String> {
    Ok(prompt(label, default, Some(explanation))?
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

/// Prompt for a single string value, returning the default when input is empty.
fn prompt(label: &str, default: Option<&str>, explanation: Option<&str>) -> Result<String, String> {
    if let Some(explanation) = explanation {
        println!("{explanation}");
    }

    match default {
        Some(default) => print!("{label} [{default}]: "),
        None => print!("{label} []: "),
    }
    io::stdout()
        .flush()
        .map_err(|error| format!("failed to flush stdout: {error}"))?;

    let mut input = String::new();
    io::stdin()
        .read_line(&mut input)
        .map_err(|error| format!("failed to read stdin: {error}"))?;

    let value = input.trim();
    if value.is_empty() {
        Ok(default.unwrap_or_default().to_owned())
    } else {
        Ok(value.to_owned())
    }
}

/// Choose the profile file to read from using the configured precedence.
///
/// Explicit paths win first, then an existing project-local file, then an
/// existing global file, and finally the project-local path as the empty default.
fn read_profile_path(explicit: Option<PathBuf>) -> Result<PathBuf, String> {
    if let Some(path) = explicit {
        return Ok(path);
    }

    let project_local = project_profile_path()?;
    if project_local.exists() {
        return Ok(project_local);
    }

    let global = global_profile_path()?;
    if global.exists() {
        return Ok(global);
    }

    Ok(project_local)
}

/// Choose the profile file to write to.
///
/// Explicit paths win first. Otherwise `--global` writes under `$DPM_HOME`, and
/// the default target is the current project's `.dpm/trace/profiles.toml`.
fn write_profile_path(explicit: Option<PathBuf>, global: bool) -> Result<PathBuf, String> {
    if let Some(path) = explicit {
        return Ok(path);
    }

    if global {
        return global_profile_path();
    }

    project_profile_path()
}

/// Return the current project's profile path.
fn project_profile_path() -> Result<PathBuf, String> {
    Ok(env::current_dir()
        .map_err(|error| format!("failed to determine current directory: {error}"))?
        .join(".dpm")
        .join("trace")
        .join("profiles.toml"))
}

/// Return the user-global profile path under `$DPM_HOME`.
fn global_profile_path() -> Result<PathBuf, String> {
    let dpm_home = env::var_os("DPM_HOME")
        .map(PathBuf::from)
        .ok_or_else(|| "DPM_HOME is not set; pass --profile-file or omit --global".to_owned())?;

    Ok(dpm_home.join("trace").join("profiles.toml"))
}

/// Read and parse a profile file, returning an empty profile set if it is absent.
fn read_profiles_if_exists(path: &PathBuf) -> Result<ProfilesFile, String> {
    if !path.exists() {
        return Ok(ProfilesFile::default());
    }

    let contents = fs::read_to_string(path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    toml::from_str(&contents)
        .map_err(|error| format!("failed to parse {}: {error}", path.display()))
}

/// Serialise profiles to TOML and write them to disk.
///
/// Parent directories are created when needed so first-time profile creation can
/// write `.dpm/trace/profiles.toml` without prior setup.
fn write_profiles(path: &PathBuf, profiles: &ProfilesFile) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    }

    let contents = toml::to_string_pretty(profiles)
        .map_err(|error| format!("failed to serialise profiles: {error}"))?;
    fs::write(path, contents)
        .map_err(|error| format!("failed to write {}: {error}", path.display()))
}
