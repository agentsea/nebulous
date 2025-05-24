use dirs;
use dotenv::dotenv;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Default, Debug)]
pub struct GlobalConfig {
    pub servers: Vec<ServerConfig>,
    pub current_server: Option<String>,
}

#[derive(Serialize, Deserialize, Default, Clone, Debug)]
pub struct ServerConfig {
    /// Optional identifier for your server config.
    pub name: Option<String>,
    pub api_key: Option<String>,
    pub server: Option<String>,
    pub auth_server: Option<String>,
}

impl GlobalConfig {
    /// Read the config from disk, or create a default one.
    /// Then ensure that we either find or create a matching server in `self.servers`
    /// based on environment variables, and set that as the `default_server`.
    pub fn read() -> Result<Self, Box<dyn std::error::Error>> {
        let config_path = get_config_file_path()?;
        let path_exists = config_path.exists();

        // Load or create default
        let mut config = if path_exists {
            let yaml = fs::read_to_string(&config_path)?;
            serde_yaml::from_str::<GlobalConfig>(&yaml)?
        } else {
            GlobalConfig::default()
        };

        // Collect environment variables (NO fallback defaults here)
        let env_api_key = env::var("NEBU_API_KEY")
            .or_else(|_| env::var("NEBULOUS_API_KEY"))
            .or_else(|_| env::var("AGENTSEA_API_KEY"))
            .ok();
        let env_server = env::var("NEBU_SERVER")
            .or_else(|_| env::var("NEBULOUS_SERVER"))
            .or_else(|_| env::var("AGENTSEA_SERVER"))
            .ok();
        let env_auth_server = env::var("NEBU_AUTH_SERVER")
            .or_else(|_| env::var("NEBULOUS_AUTH_SERVER"))
            .or_else(|_| env::var("AGENTSEA_AUTH_SERVER"))
            .ok();

        // Only proceed if all three environment variables are present.
        if let (Some(env_api_key), Some(env_server), Some(env_auth_server)) =
            (env_api_key, env_server, env_auth_server)
        {
            // Find a matching server (all three fields match).
            let found_server = config.servers.iter_mut().find(|srv| {
                srv.api_key.as_deref() == Some(&env_api_key)
                    && srv.server.as_deref() == Some(&env_server)
                    && srv.auth_server.as_deref() == Some(&env_auth_server)
            });

            // If found, use that. If not, create a new entry.
            let server_name = "env-based-server".to_string();
            let chosen_name = if let Some(srv) = found_server {
                // Make sure it has a name, so we can set default_server to it
                if srv.name.is_none() {
                    srv.name = Some(server_name.clone());
                }
                srv.name.clone().unwrap()
            } else {
                // Need to create a new server entry
                let new_server = ServerConfig {
                    name: Some(server_name.clone()),
                    api_key: Some(env_api_key),
                    server: Some(env_server),
                    auth_server: Some(env_auth_server),
                };
                config.servers.push(new_server);
                server_name
            };

            // Set that server as the "current" or default
            config.current_server = Some(chosen_name);
        }

        // Only write if the file didn't already exist
        if !path_exists {
            config.write()?;
        }

        Ok(config)
    }

    /// Write the current GlobalConfig to disk (YAML).
    pub fn write(&self) -> Result<(), Box<dyn std::error::Error>> {
        let config_path = get_config_file_path()?;

        // Create parent directories if they don't exist
        if let Some(parent) = config_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let yaml = serde_yaml::to_string(self)?;
        fs::write(config_path, yaml)?;

        Ok(())
    }

    /// Get the server config for the current `default_server`.
    /// Returns `None` if `default_server` is unset or if no server
    /// with that name is found.
    pub fn get_current_server_config(&self) -> Option<&ServerConfig> {
        self.current_server.as_deref().and_then(|name| {
            self.servers
                .iter()
                .find(|srv| srv.name.as_deref() == Some(name))
        })
    }

    pub fn get_auth_server(&self) -> Option<&str> {
        self.get_current_server_config()
            .and_then(|cfg| cfg.auth_server.as_deref())
            .or_else(|| Some(CONFIG.auth_server.as_str()))
    }
}

fn get_config_file_path() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let home_dir = dirs::home_dir().ok_or("Could not determine home directory")?;
    let config_dir = home_dir.join(".agentsea");
    let config_path = config_dir.join("nebu.yaml");
    Ok(config_path)
}

#[derive(Debug, Clone)]
pub struct Config {
    pub message_queue_type: String,
    pub kafka_bootstrap_servers: String,
    pub kafka_timeout_ms: String,
    pub redis_host: String,
    pub redis_port: String,
    pub redis_password: Option<String>,
    pub redis_url: Option<String>,
    pub redis_publish_url: Option<String>,
    pub database_url: String,
    pub tailscale_api_key: Option<String>,
    pub tailscale_tailnet: Option<String>,
    pub bucket_name: String,
    pub bucket_region: String,
    pub root_owner: String,
    pub auth_server: String,
    pub publish_url: Option<String>,
}

impl Config {
    pub fn new() -> Self {
        dotenv().ok();

        Self {
            message_queue_type: env::var("MESSAGE_QUEUE_TYPE")
                .unwrap_or_else(|_| "redis".to_string()),
            kafka_bootstrap_servers: env::var("KAFKA_BOOTSTRAP_SERVERS")
                .unwrap_or_else(|_| "localhost:9092".to_string()),
            kafka_timeout_ms: env::var("KAFKA_TIMEOUT_MS").unwrap_or_else(|_| "5000".to_string()),
            redis_host: env::var("REDIS_HOST").unwrap_or_else(|_| "127.0.0.1".to_string()),
            redis_port: env::var("REDIS_PORT").unwrap_or_else(|_| "6379".to_string()),
            redis_password: env::var("REDIS_PASSWORD").ok(),
            redis_url: env::var("REDIS_URL").ok(),
            redis_publish_url: env::var("REDIS_PUBLISH_URL").ok(),
            database_url: env::var("DATABASE_URL")
                .unwrap_or_else(|_| "sqlite://.data/data.db".to_string()),
            tailscale_api_key: env::var("TS_API_KEY").or_else(|_| env::var("TAILSCALE_API_KEY")).ok(),
            tailscale_tailnet: env::var("TAILSCALE_TAILNET").ok(),
            bucket_name: env::var("NEBU_BUCKET_NAME")
                .or_else(|_| env::var("NEBULOUS_BUCKET_NAME"))
                .unwrap_or_else(|_| panic!("NEBU_BUCKET_NAME or NEBULOUS_BUCKET_NAME environment variable must be set")),
            bucket_region: env::var("NEBU_BUCKET_REGION")
                .or_else(|_| env::var("NEBULOUS_BUCKET_REGION"))
                .unwrap_or_else(|_| panic!("NEBU_BUCKET_REGION or NEBULOUS_BUCKET_REGION environment variable must be set")),
            root_owner: env::var("NEBU_ROOT_OWNER")
                .or_else(|_| env::var("NEBULOUS_ROOT_OWNER"))
                .unwrap_or_else(|_| panic!("NEBU_ROOT_OWNER or NEBULOUS_ROOT_OWNER environment variable must be set")),
            auth_server: env::var("NEBU_AUTH_SERVER")
                .or_else(|_| env::var("NEBULOUS_AUTH_SERVER"))
                .or_else(|_| env::var("AGENTSEA_AUTH_SERVER"))
                .or_else(|_| env::var("AGENTSEA_AUTH_URL"))
                .unwrap_or_else(|_| "https://auth.hub.agentlabs.xyz".to_string()),
            publish_url: env::var("NEBU_PUBLISH_URL")
                .or_else(|_| env::var("NEBULOUS_PUBLISH_URL"))
                .ok(),
        }
    }
}
// Global static CONFIG instance
pub static CONFIG: Lazy<Config> = Lazy::new(Config::new);
