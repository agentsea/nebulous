use std::error::Error;
use std::io::{self, Write};

use crate::commands::request::server_request;
use nebulous::config::{ClientConfig, ClientServerConfig};
use nebulous::models::V1UserProfile;
use open;
use rpassword;

pub async fn execute(
    nebu_url: String,
    name: String,
    update: bool,
    auth: Option<String>,
    hub: Option<String>,
) -> Result<(), Box<dyn Error>> {
    if auth.is_none() ^ hub.is_none() {
        eprintln!("Either auth or hub URL provided. Please provide both or neither.");
        return Ok(());
    }

    let nebu_url = nebu_url.trim().trim_end_matches("/").to_string();

    let mut config = ClientConfig::read()?;

    if config.contains_server(&name) && !update {
        eprintln!("Server with name '{}' already exists. Please choose a different name or set --update flag.", name);
        return Ok(());
    }

    if auth.is_some() && hub.is_some() {
        let auth_url = auth.unwrap().trim().trim_end_matches("/").to_string();
        let hub_url = hub.unwrap().trim().trim_end_matches("/").to_string();

        let hub_api_url = format!("{}/settings/api", hub_url);
        println!("\nVisit {} to get an API key\n", hub_api_url);

        // Attempt to open the URL in the default browser
        if let Err(e) = open::that(&hub_api_url) {
            eprintln!("Failed to open browser: {}", e);
        }

        print!("Enter your API key: ");
        io::stdout().flush()?;
        let api_key = rpassword::read_password()?;

        config.update_server(
            ClientServerConfig {
                name,
                server: Some(nebu_url),
                api_key: Some(api_key),
                auth_server: Some(auth_url),
            },
            true,
        );
    } else {
        println!(
            r#"Configuring the Nebulous CLI to use the integrated auth server.
To obtain an API key, execute the following command within the container:

    nebulous auth api-keys generate

When you're running nebulous on Docker, use:

    docker exec -it <container_id> nebulous auth api-keys generate

When you're running nebulous on Kubernetes, use:

    kubectl exec -it <pod_name> -- nebulous auth api-keys generate
"#
        );

        print!("Enter your API key: ");
        io::stdout().flush()?;
        let api_key = rpassword::read_password()?;

        config.update_server(
            ClientServerConfig {
                name,
                server: Some(nebu_url),
                api_key: Some(api_key),
                auth_server: None,
            },
            true,
        );
    }

    let response = server_request("/v1/users/me", reqwest::Method::GET).await?;
    let profile: V1UserProfile = response.json().await?;
    println!(
        "\nSuccessfully logged into '{}' as '{}'",
        config.current_server.clone().unwrap(),
        profile.email
    );

    config.write()?;
    Ok(())
}
