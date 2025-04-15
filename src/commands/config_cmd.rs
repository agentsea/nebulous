use std::error::Error;

pub async fn show_config() -> Result<(), Box<dyn Error>> {
    match nebulous::config::ClientConfig::read() {
        Ok(config) => match serde_yaml::to_string(&config) {
            Ok(yaml) => println!("{}", yaml),
            Err(e) => eprintln!("Error formatting config as YAML: {}", e),
        },
        Err(e) => {
            eprintln!("Error reading config: {}", e);
        }
    }
    Ok(())
}

pub async fn show_current() -> Result<(), Box<dyn Error>> {
    match nebulous::config::ClientConfig::read() {
        Ok(config) => {
            if let Some(current_server) = config.get_current_server_config() {
                let mut current_server = current_server.clone();
                current_server.api_key = Some("<redacted>".to_string());
                match serde_yaml::to_string(&current_server) {
                    Ok(yaml) => println!("{}", yaml),
                    Err(e) => eprintln!("Error formatting current server config as YAML: {}", e),
                }
            } else {
                eprintln!("No current server configuration found.");
            }
        }
        Err(e) => {
            eprintln!("Error reading config: {}", e);
        }
    }
    Ok(())
}

pub async fn set_current(name: &str) -> Result<(), Box<dyn Error>> {
    match nebulous::config::ClientConfig::read() {
        Ok(mut config) => {
            if let Some(server_config) = config.get_server(name) {
                config.current_server = Some(server_config.name.clone());
                match config.write() {
                    Ok(_) => println!("Current server set to '{}'", name),
                    Err(e) => eprintln!("Error writing config: {}", e),
                }
            } else {
                eprintln!("Server '{}' not found in configuration.", name);
            }
        }
        Err(e) => {
            eprintln!("Error reading config: {}", e);
        }
    }
    Ok(())
}
