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
    pub name: String,
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

        // Only write if the file didn't already exist
        if !path_exists {
            config.write()?;
        }

        config.create_config_from_environment();

        Ok(config)
    }

    fn create_config_from_environment(&mut self) {
        let env_api_key = env::var("NEBU_API_KEY")
            .or_else(|_| env::var("AGENTSEA_API_KEY"))
            .ok();
        let env_server = env::var("NEBU_SERVER")
            .or_else(|_| env::var("AGENTSEA_SERVER"))
            .ok();
        let env_auth_server = env::var("NEBU_AUTH_SERVER")
            .or_else(|_| env::var("AGENTSEA_AUTH_SERVER"))
            .ok();

        if let (Some(env_api_key), Some(env_server), Some(env_auth_server)) =
            (env_api_key, env_server, env_auth_server)
        {
            // Find a matching server (all three fields match).
            let found_server = self.servers.iter_mut().find(|srv| {
                srv.api_key.as_deref() == Some(&env_api_key)
                    && srv.server.as_deref() == Some(&env_server)
                    && srv.auth_server.as_deref() == Some(&env_auth_server)
            });

            // If found, use that. If not, create a new entry.
            if let Some(srv) = found_server {
                self.current_server = Some(srv.name.clone());
            } else {
                let new_server = ServerConfig {
                    name: "env-based-server".to_string(),
                    api_key: Some(env_api_key),
                    server: Some(env_server),
                    auth_server: Some(env_auth_server),
                };
                self.update_server(new_server, true);
            };
        }
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

    /// Get the server config for the current server.
    pub fn get_current_server_config(&self) -> Option<&ServerConfig> {
        self.current_server
            .as_deref()
            .and_then(|name| self.servers.iter().find(|srv| srv.name == name))
    }

    /// Get the server config for a specific server.
    pub fn get_server(&self, name: &str) -> Option<&ServerConfig> {
        self.servers.iter().find(|srv| srv.name == name)
    }

    /// Remove a server from the config.
    pub fn drop_server(&mut self, name: &str) {
        if let Some(pos) = self.servers.iter().position(|srv| srv.name == name) {
            self.servers.remove(pos);

            // If the removed server was the current one, clear it.
            if self.current_server == Some(name.to_string()) {
                self.current_server = None;
            }
        }
    }

    /// Update or add a server config.
    pub fn update_server(&mut self, new_config: ServerConfig, make_current: bool) {
        if let Some(pos) = self
            .servers
            .iter()
            .position(|srv| srv.name == new_config.name)
        {
            self.servers[pos] = new_config;
        } else {
            if make_current {
                self.current_server = Some(new_config.name.clone());
            }
            self.servers.push(new_config);
        }
    }

    /// Add a server.
    pub fn add_server(&mut self, config: ServerConfig, make_current: bool) {
        if self.contains_server(&config.name) {
            eprintln!(
                "Server with name '{}' already exists. Please choose a different name.",
                config.name
            );
            return;
        }
        self.update_server(config, make_current);
    }

    /// Check if a server with the given name exists.
    pub fn contains_server(&self, name: &str) -> bool {
        self.servers.iter().any(|srv| srv.name == name)
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
    pub database_url: String,
    pub tailscale_api_key: Option<String>,
    pub tailscale_tailnet: Option<String>,
    pub bucket_name: String,
    pub bucket_region: String,
    pub root_owner: String,
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
            database_url: env::var("DATABASE_URL")
                .unwrap_or_else(|_| "sqlite://.data/data.db".to_string()),
            tailscale_api_key: env::var("TAILSCALE_API_KEY").ok(),
            tailscale_tailnet: env::var("TAILSCALE_TAILNET").ok(),
            bucket_name: env::var("NEBU_BUCKET_NAME")
                .unwrap_or_else(|_| panic!("NEBU_BUCKET_NAME environment variable must be set")),
            bucket_region: env::var("NEBU_BUCKET_REGION")
                .unwrap_or_else(|_| panic!("NEBU_BUCKET_REGION environment variable must be set")),
            root_owner: env::var("NEBU_ROOT_OWNER")
                .unwrap_or_else(|_| panic!("NEBU_ROOT_OWNER environment variable must be set")),
        }
    }
}
// Global static CONFIG instance
pub static CONFIG: Lazy<Config> = Lazy::new(Config::new);
