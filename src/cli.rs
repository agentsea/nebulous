use clap::{arg, ArgAction, Args, Parser, Subcommand};

/// Nebulous CLI
#[derive(Parser)]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

/// The subcommands supported by the CLI.
#[derive(Subcommand)]
pub enum Commands {
    /// Create resources.
    Create {
        #[command(subcommand)]
        command: CreateCommands,
    },

    /// Get resources.
    Get {
        #[command(subcommand)]
        command: GetCommands,
    },

    /// Delete resources.
    Delete {
        #[command(subcommand)]
        command: DeleteCommands,
    },

    /// Sync data
    Sync {
        #[command(subcommand)]
        command: SyncCommands,
    },

    /// Select a checkpoint.
    Select {
        #[command(subcommand)]
        command: SelectCommands,
    },

    /// Serve the API.
    Serve {
        /// The address to bind to. Ignored when launching through docker.
        #[arg(long, default_value = "127.0.0.1")]
        host: String,

        /// The port to bind to.
        #[arg(long, default_value_t = 3000)]
        port: u16,

        /// Disable the internal auth server
        #[arg(long, default_value_t = false)]
        disable_internal_auth: bool,

        /// The port to bind the internal auth server to.
        #[arg(long, default_value_t = 8080)]
        auth_port: u16,

        /// Launch through Docker
        #[arg(long, default_value_t = false)]
        docker: bool,
    },

    /// Proxy services.
    Proxy {
        #[command(subcommand)]
        command: ProxyCommands,
    },

    /// Run the daemon.
    Daemon {
        /// The address to bind to.
        #[arg(long, default_value = "127.0.0.1")]
        host: String,

        /// The port to bind to.
        #[arg(short, long, default_value_t = 3000)]
        port: u16,

        /// Run in the background (detached) if true.
        #[arg(short, long, default_value_t = false)]
        background: bool,
    },

    /// Fetch logs for a container.
    Logs {
        /// Container name.
        name: String,

        /// Container namespace.
        #[arg(long, short)]
        namespace: String,
    },

    /// Login to a Nebulous API server.
    Login {
        /// Address of the API server
        #[arg(default_value = "http://127.0.0.1")]
        url: String,

        /// Name of the server
        #[arg(long, default_value = "nebulous")]
        name: String,

        /// Update the server if it already exists
        #[arg(long, default_value_t = false)]
        update: bool,

        /// Address of the Auth server
        #[arg(long, default_value = None)]
        auth: Option<String>,

        /// Address of the Hub
        #[arg(long, default_value = None)]
        hub: Option<String>,
    },

    /// Execute a command inside a container.
    Exec(ExecArgs),

    /// Manage authentication
    Auth {
        #[command(subcommand)]
        command: AuthCommands,
    },

    /// Manage configuration
    Config {
        /// The action to perform.
        #[command(subcommand)]
        command: ConfigCommands,
    },
}

/// Select a checkpoint.
#[derive(Subcommand)]
pub enum SelectCommands {
    /// Select a checkpoint from a base directory using a given criterion.
    Checkpoint {
        /// Path to the base directory holding checkpoints (e.g., "checkpoint-1", "checkpoint-2").
        #[arg(long, default_value = ".")]
        base_dir: String,

        /// The selection criterion: "latest" or "best".
        #[arg(long, default_value = "best")]
        criteria: String,
    },
}

// The struct that captures all CLI fields for the Exec command.
#[derive(Args)]
pub struct ExecArgs {
    /// Container's name
    pub name: String,

    /// Container's namespace
    #[arg(long, short)]
    pub namespace: String,

    /// The command (and args) to run in the container
    #[arg(long, short)]
    pub command: String,

    /// Whether to pass `-i` (interactive)
    #[arg(short = 'i', long, default_value_t = false)]
    pub interactive: bool,

    /// Whether to pass `-t` (tty)
    #[arg(short = 't', long, default_value_t = false)]
    pub tty: bool,
}

/// Secret creation parameters
#[derive(Args)]
pub struct SecretCommands {
    /// Secret name
    pub name: String,

    /// Secret namespace
    #[arg(long)]
    pub namespace: Option<String>,

    /// The secret value. (If none given, you must provide a file instead)
    #[arg(long)]
    pub value: Option<String>,

    /// Time (in seconds from epoch or similar) for the secret to expire
    #[arg(long)]
    pub expires_at: Option<i32>,

    /// Read the secret value from a file.
    #[arg(short = 'f', long)]
    pub file: Option<String>,
}

#[derive(Subcommand)]
pub enum ProxyCommands {
    /// Proxy local shell commands over HTTP.
    Shell {
        /// The host to bind to.
        #[arg(long, default_value = "0.0.0.0")]
        host: String,

        /// The port to bind to.
        #[arg(short, long, default_value_t = 8080)]
        port: u16,
    },
}

/// Subcommands for syncing
#[derive(Subcommand)]
pub enum SyncCommands {
    /// Sync a volume.
    #[command(aliases = ["volume", "vol"])]
    Volumes {
        /// Path to the YAML configuration file.
        #[arg(short, long)]
        config: String,

        /// Interval in seconds to sync.
        #[arg(short, long, default_value_t = 60)]
        interval_seconds: u64,

        /// Create the config file if it doesn't exist.
        #[arg(long, default_value_t = false)]
        create_if_missing: bool,

        /// Run in the background.
        #[arg(short, long, default_value_t = false)]
        watch: bool,

        /// Run in the background.
        #[arg(short, long, default_value_t = false)]
        background: bool,

        /// Block until the one time sync paths are complete.
        #[arg(long, default_value_t = false)]
        block_once: bool,

        /// Sync from the NEBU_SYNC_CONFIG environment variable.
        #[arg(long, default_value_t = false)]
        config_from_env: bool,
    },

    /// Ensure all syncs are complete.
    Wait {
        /// Path to the YAML configuration file.
        #[arg(short, long)]
        config: String,

        /// Interval in seconds to sync.
        #[arg(short, long, default_value_t = 2)]
        interval_seconds: u64,
    },
}

/// Create resources.
#[derive(Subcommand)]
pub enum CreateCommands {
    /// Create a container.
    #[command(aliases = ["container", "co"])]
    Containers {
        #[command(flatten)]
        command: ContainerCommands,
    },

    /// Create a secret.
    #[command(aliases = ["secret", "sec"])]
    Secrets {
        #[command(flatten)]
        command: SecretCommands,
    },
}

/// Container creation parameters
#[derive(Args)]
pub struct ContainerCommands {
    /// Container name
    #[arg(long)]
    pub name: Option<String>,

    /// Container namespace
    #[arg(long)]
    pub namespace: Option<String>,

    /// Platform to run the container on
    #[arg(long)]
    pub platform: Option<String>,

    /// Container image
    #[arg(long)]
    pub image: Option<String>,

    /// Command to run in the container
    #[arg(long)]
    pub cmd: Option<String>,

    /// Environment variables in KEY=VALUE format
    #[arg(long, value_parser = parse_key_val, action = ArgAction::Append)]
    pub env: Option<Vec<(String, String)>>,

    /// Labels in KEY=VALUE format
    #[arg(long, value_parser = parse_key_val, action = ArgAction::Append)]
    pub label: Option<Vec<(String, String)>>,

    /// Accelerators to use
    #[arg(long, action = ArgAction::Append)]
    pub accelerators: Option<Vec<String>>,

    /// Source path for volume mount
    #[arg(long)]
    pub volume_source: Option<String>,

    /// Destination path for volume mount
    #[arg(long)]
    pub volume_destination: Option<String>,

    /// Enable bidirectional sync for volume (default: true)
    #[arg(long, default_value = "RCLONE_SYNC")]
    pub volume_type: Option<String>,

    /// Enable continuous sync for volume (default: true)
    #[arg(long, default_value_t = true)]
    pub volume_continuous: bool,

    /// Enable resync for volume (default: false)
    #[arg(long, default_value_t = false)]
    pub volume_resync: bool,

    /// Cache directory for volume (default: /nebu/cache)
    #[arg(long, default_value = "/nebu/cache")]
    pub volume_cache_dir: String,

    /// File input
    #[arg(short = 'f', long)]
    pub file: Option<String>,

    /// Meters of the container
    #[arg(long)]
    pub meter_cost: Option<f64>,

    /// Meter cost plus
    #[arg(long)]
    pub meter_cost_plus: Option<f64>,

    /// Meter unit of the container
    #[arg(long)]
    pub meter_metric: Option<String>,

    /// Meter currency of the container
    #[arg(long)]
    pub meter_currency: Option<String>,

    /// Meter unit of the container
    #[arg(long)]
    pub meter_unit: Option<String>,

    /// Restart policy of the container
    #[arg(long)]
    pub restart: Option<String>,

    /// Queue to run the container in
    #[arg(long)]
    pub queue: Option<String>,

    /// Timeout for the container
    #[arg(long)]
    pub timeout: Option<String>,

    /// Minimum CPU
    #[arg(long)]
    pub min_cpu: Option<f64>,

    /// Minimum memory
    #[arg(long)]
    pub min_memory: Option<f64>,

    /// Maximum CPU
    #[arg(long)]
    pub max_cpu: Option<f64>,

    /// Maximum memory
    #[arg(long)]
    pub max_memory: Option<f64>,

    /// Proxy port
    #[arg(long)]
    pub proxy_port: Option<i16>,
}

/// Parse a key-value pair in the format of KEY=VALUE
fn parse_key_val(s: &str) -> Result<(String, String), String> {
    let pos = s
        .find('=')
        .ok_or_else(|| format!("Invalid KEY=VALUE: no `=` found in `{s}`"))?;
    Ok((s[..pos].to_string(), s[pos + 1..].to_string()))
}

/// Get resources.
#[derive(Subcommand)]
pub enum GetCommands {
    /// Get accelerators.
    #[command(aliases = ["accelerator", "acc"])]
    Accelerators {
        /// Platform to get accelerators for.
        #[arg(long)]
        platform: Option<String>,
    },

    /// Get containers.
    #[command(aliases = ["container", "co"])]
    Containers {
        /// Platform to get containers for.
        id: Option<String>,
    },

    /// Get platforms.
    #[command(aliases = ["platform", "plat"])]
    Platforms,

    /// Get secrets.
    #[command(aliases = ["secret", "sec"])]
    Secrets {
        /// Optional secret ID.
        id: Option<String>,
    },
}

/// Delete resources.
#[derive(Subcommand)]
pub enum DeleteCommands {
    /// Delete a container.
    #[command(aliases = ["container", "co"])]
    Containers {
        /// ID.
        id: String,
    },
}

/// Subcommands for the "work" command
#[derive(Subcommand)]
pub enum WorkCommands {}

/// Subcommands for the "auth" command
#[derive(Subcommand)]
pub enum AuthCommands {
    /// Manage API keys for authentication with the Nebulous API server.
    #[command(aliases = ["api-key"])]
    ApiKeys {
        #[command(subcommand)]
        action: ApiKeyActions,
    },
    // TODO: Add auth for tailnet
}

#[derive(Subcommand)]
pub enum ApiKeyActions {
    /// List API keys.
    List,

    /// Get API key details.
    Get {
        /// The ID of the API key to get.
        id: String,
    },

    /// Generate a new API key.
    Generate,

    /// Revoke an API key.
    Revoke {
        /// The ID of the API key to delete.
        id: String,
    },
}

#[derive(Subcommand)]
pub enum ConfigCommands {
    /// Show the full configuration
    Show,

    /// Manage the current configuration
    Current {
        /// The action to perform.
        #[command(subcommand)]
        action: CurrentConfigActions,
    },
}

#[derive(Subcommand)]
pub enum CurrentConfigActions {
    /// Show the current configuration
    Show,
    /// Set the current configuration
    Set {
        /// The name of the server to set as current
        server: String,
    },
}
