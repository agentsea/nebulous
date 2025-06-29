use colored::Colorize;
use nebulous::config::ClientConfig;
use std::error::Error;

pub async fn set_context(server_name: &str) -> Result<(), Box<dyn Error>> {
    // Read the current config
    let mut config = ClientConfig::read()?;

    // Check if the server exists
    let server_exists = config
        .servers
        .iter()
        .any(|s| s.name == server_name);

    if !server_exists {
        return Err(format!("Server '{}' not found in configuration", server_name).into());
    }

    // Update the current_server field
    config.current_server = Some(server_name.to_string());

    // Write the updated config back to disk
    config.write()?;

    println!(
        "{} {}",
        "Current context set to:".green(),
        server_name.bold()
    );

    Ok(())
}
