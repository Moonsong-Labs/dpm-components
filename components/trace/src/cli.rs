use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use crate::config::AuthMode;

#[derive(Debug, Parser)]
#[command(
    name = "trace",
    version,
    about = "Inspect Daml ledger transactions",
    long_about = "Inspect Daml ledger transactions through the participant Ledger API."
)]
pub struct Cli {
    /// Optional message.
    #[arg(long, default_value = "Be a darling and give me a 'Hello there'")]
    pub message: String,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Manage trace connection profiles.
    Profile {
        #[command(subcommand)]
        command: ProfileCommand,
    },
    /// Authenticate against the OAuth2 issuer configured for a profile.
    Login(ProfileSelector),
    /// Remove locally stored tokens for a profile.
    Logout(ProfileSelector),
}

#[derive(Debug, Args)]
pub struct ProfileSelector {
    /// Profile name.
    #[arg(long, default_value = "default")]
    pub profile: String,

    /// Use this profile file instead of the default lookup locations.
    #[arg(long)]
    pub profile_file: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
pub enum ProfileCommand {
    /// Add or update a trace profile.
    Add(ProfileAddArgs),
    /// List known trace profiles.
    List(ProfileFileArgs),
    /// Show a trace profile.
    Show(ProfileShowArgs),
}

#[derive(Debug, Args)]
pub struct ProfileAddArgs {
    /// Profile name.
    pub name: String,

    /// Write to this profile file instead of the default target.
    #[arg(long)]
    pub profile_file: Option<PathBuf>,

    /// Write to $DPM_HOME/trace/profiles.toml instead of the project-local file.
    #[arg(long)]
    pub global: bool,

    /// Ledger API address, for example localhost:6865.
    #[arg(long)]
    pub ledger: Option<String>,

    /// Use TLS for Ledger API connections.
    #[arg(long, conflicts_with = "plaintext")]
    pub tls: bool,

    /// Use plaintext Ledger API connections.
    #[arg(long, conflicts_with = "tls")]
    pub plaintext: bool,

    /// Authentication mode for Ledger API requests.
    #[arg(long, value_enum, default_value = "remote")]
    pub auth_mode: AuthMode,

    /// OAuth2 issuer URL.
    #[arg(long)]
    pub issuer: Option<String>,

    /// OAuth2 client id.
    #[arg(long)]
    pub client_id: Option<String>,

    /// OAuth2 audience.
    #[arg(long)]
    pub audience: Option<String>,

    /// OAuth2 scope. Can be passed more than once.
    #[arg(long = "scope")]
    pub scopes: Vec<String>,

    /// Default party. Can be passed more than once.
    #[arg(long = "party")]
    pub parties: Vec<String>,
}

#[derive(Debug, Args)]
pub struct ProfileFileArgs {
    /// Use this profile file instead of the default lookup locations.
    #[arg(long)]
    pub profile_file: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct ProfileShowArgs {
    /// Profile name.
    pub name: String,

    /// Use this profile file instead of the default lookup locations.
    #[arg(long)]
    pub profile_file: Option<PathBuf>,
}
