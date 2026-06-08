use clap::{Parser, Subcommand};

/// CLI for installing and running the Hytale game client.
#[derive(Parser)]
#[command(version, about)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Authenticate with Hytale OAuth.
    Auth {
        #[command(subcommand)]
        sub: AuthCommand,
    },

    /// Manage accounts.
    Account {
        #[command(subcommand)]
        sub: AccountCommand,
    },

    /// Manage profiles.
    Profile {
        #[command(subcommand)]
        sub: ProfileCommand,
    },

    /// Manage game versions.
    Version {
        #[command(subcommand)]
        sub: VersionCommand,
    },

    /// Download game client assets for a version.
    Install {
        /// Version to install (e.g. 1.0.0, 0.6.0-pre.2), or channel name for latest
        /// (release, pre-release). Defaults to latest release.
        version: Option<String>,

        /// Target output directory. Defaults to data dir.
        #[arg(short, long)]
        output: Option<String>,
    },

    /// Launch the game client.
    Run {
        /// Profile UUID or username to launch with.
        profile: Option<String>,

        /// Account label to use.
        #[arg(short, long)]
        account: Option<String>,

        /// Version to run (e.g. 1.0.0) or channel name for latest (release, pre-release).
        #[arg(short, long)]
        version: Option<String>,

        /// Detach immediately after launch without waiting for the game to exit.
        #[arg(short, long)]
        detach: bool,

        /// Extra JVM arguments.
        #[arg(last = true)]
        extra_args: Vec<String>,
    },

    /// Manage and run the game server.
    Server {
        #[command(subcommand)]
        sub: ServerCommand,
    },

}

#[derive(Subcommand)]
pub enum AuthCommand {
    /// Authenticate via browser (PKCE flow). Downloads and launches the game.
    Login {
        /// Account label for local storage.
        #[arg(short, long)]
        label: Option<String>,
    },


    /// Refresh the access token for an account.
    Refresh {
        /// Account label to refresh.
        account: String,
    },

    /// Log out (remove stored tokens).
    Logout {
        /// Account label to remove.
        account: String,
    },

    /// Show current auth status.
    Status,
}

#[derive(Subcommand)]
pub enum AccountCommand {
    /// List configured accounts.
    List,

    /// Set a default account.
    Default {
        /// Account label to set as default.
        account: String,
    },

    /// Remove an account.
    Remove {
        /// Account label to remove.
        account: String,
    },
}

#[derive(Subcommand)]
pub enum ProfileCommand {
    /// List profiles across all accounts.
    List {
        /// Account label to list profiles for.
        #[arg(short, long)]
        account: Option<String>,
    },

    /// Set a default profile for an account.
    Default {
        /// Profile username or UUID.
        profile: String,

        /// Account label.
        #[arg(short, long)]
        account: Option<String>,
    },

    /// Fetch fresh launcher data for an account.
    Refresh {
        /// Account label.
        account: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum VersionCommand {
    /// List available versions.
    List {
        /// Patchline channel: release or pre-release.
        #[arg(short, long, default_value = "release")]
        channel: String,
    },

    /// Set a default version.
    Default {
        /// Version string (e.g. 1.0.0).
        version: String,
    },

    /// Show currently installed versions.
    Installed,

    /// Remove a downloaded version.
    Remove {
        /// Version string (e.g. 1.0.0).
        version: String,
    },
}


#[derive(Subcommand)]
pub enum ServerCommand {
    /// Launch the game server.
    Run {
        /// Version to run (e.g. 1.0.0) or channel name for latest (release, pre-release).
        #[arg(short, long)]
        version: Option<String>,

        /// Detach immediately after launch without waiting for the server to exit.
        #[arg(short, long)]
        detach: bool,

        /// Extra JVM arguments.
        #[arg(last = true)]
        extra_args: Vec<String>,
    },
}
