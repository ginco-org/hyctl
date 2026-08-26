use clap::{Parser, Subcommand};

/// hyctl — Hytale launcher
#[derive(Parser)]
#[command(name = "hyctl", version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,

    /// Disable ANSI color output
    #[arg(long, global = true)]
    pub no_color: bool,
}

#[derive(Subcommand)]
pub enum Command {
    /// Launch the game client
    Launch {
        /// Profile to launch with (default: default account's default profile)
        #[arg(short, long, value_name = "name")]
        profile: Option<String>,

        /// Game version to launch (default: latest)
        #[arg(short, long, value_name = "ver")]
        version: Option<String>,

        /// Run in the background; return immediately after launch
        #[arg(short, long)]
        background: bool,

        /// Connect to the specified server
        #[arg(short, long, value_name = "server")]
        server: Option<String>,

        /// Load the specified world
        #[arg(short, long, value_name = "world")]
        world: Option<String>,

        /// Extra arguments passed to the game client
        #[arg(last = true)]
        extra_args: Vec<String>,
    },

    /// Run a game server
    Serve {
        /// Profile to use (default: default account's default profile)
        #[arg(short, long, value_name = "name")]
        profile: Option<String>,

        /// Server data directory
        #[arg(short, long, value_name = "path", default_value = "./server")]
        dir: String,

        /// Game version to run (default: latest)
        #[arg(short, long, value_name = "ver")]
        version: Option<String>,

        /// Custom assets path (directory or zip) to use instead of the installed Assets.zip
        #[arg(short = 'a', long, value_name = "path")]
        assets: Option<String>,

        /// Run in the background; return immediately after the server starts
        #[arg(short, long)]
        background: bool,

        /// Arguments passed directly to the server process
        #[arg(last = true)]
        extra_args: Vec<String>,
    },

    /// Manage accounts and profiles
    Auth {
        #[command(subcommand)]
        sub: AuthCommand,
    },

    /// Manage installed game versions
    Asset {
        #[command(subcommand)]
        sub: AssetCommand,
    },
}

#[derive(Subcommand)]
pub enum AuthCommand {
    /// List saved accounts and their profiles
    List,

    /// Add an account (opens browser for login)
    Add {
        /// Authenticate as a dedicated server using the hytale-server OAuth client
        /// (device code flow; grants the `auth:server` scope required by `hyctl serve`)
        #[arg(long)]
        server: bool,
    },

    /// Remove a saved account
    Remove {
        /// Account label to remove
        account: String,
    },

    /// Set the default account
    Default {
        /// Account label to set as default
        account: String,
    },
}

#[derive(Subcommand)]
pub enum AssetCommand {
    /// List installed game versions
    List,

    /// Download and install a game version
    Install {
        /// Version to install, or channel name for latest (release, pre-release)
        version: Option<String>,
    },

    /// Remove an installed version
    Remove {
        /// Version string to remove
        version: String,
    },

    /// Verify integrity of an installed version
    Verify {
        /// Version string to verify
        version: String,
    },

    /// Remove all versions except the latest
    Prune,
}
