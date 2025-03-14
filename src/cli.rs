use clap::{arg, ArgAction, Args, Parser, Subcommand};

/// Orign CLI.
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

    /// Sync a volume.
    Sync {
        /// Path to the YAML configuration file.
        #[arg(short, long)]
        config: String,

        /// Interval in seconds to sync.
        #[arg(short, long, default_value_t = 60)]
        interval_seconds: u64,

        /// Create the config file if it doesn't exist.
        #[arg(short, long, default_value_t = false)]
        create_if_missing: bool,

        /// Run in the background.
        #[arg(short, long, default_value_t = false)]
        watch: bool,

        /// Run in the background.
        #[arg(short, long, default_value_t = false)]
        background: bool,

        /// Block until the one time sync paths are complete.
        #[arg(short, long, default_value_t = false)]
        block_once: bool,

        /// Sync from the NEBU_SYNC_CONFIG environment variable.
        #[arg(short, long, default_value_t = false)]
        config_from_env: bool,
    },

    /// Serve the API server.
    Serve {
        /// The address to bind to.
        #[arg(long, default_value = "127.0.0.1")]
        host: String,

        /// The port to bind to.
        #[arg(short, long, default_value_t = 3000)]
        port: u16,
    },

    /// Login to nebu.
    Login,
}

/// Create resources.
#[derive(Subcommand)]
pub enum CreateCommands {
    /// Create a container.
    Container {
        #[command(flatten)]
        command: ContainerCommands,
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
    #[arg(long, default_value_t = true)]
    pub volume_bidirectional: bool,

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
    Accelerators {
        /// Platform to get accelerators for.
        #[arg(long)]
        platform: Option<String>,
    },

    /// Get containers.
    Containers {
        /// Platform to get containers for.
        id: Option<String>,
    },

    /// Get platforms.
    Platforms,
}

/// Delete resources.
#[derive(Subcommand)]
pub enum DeleteCommands {
    /// Delete a container.
    Containers {
        /// ID.
        id: String,
    },
}

/// Subcommands for the "work" command
#[derive(Subcommand)]
pub enum WorkCommands {}
