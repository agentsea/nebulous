use std::error::Error;
use std::io::{self, Write};

use nebulous::config::{ClientConfig, ClientServerConfig};
use open;
use rpassword;

pub async fn execute(
    nebu_url: String,
    auth: Option<String>,
    hub: Option<String>,
) -> Result<(), Box<dyn Error>> {
    if auth.is_none() ^ hub.is_none() {
        eprintln!("Either auth or hub URL provided. Please provide both or neither.");
        return Ok(());
    }

    let nebu_url = nebu_url.trim().trim_end_matches("/").to_string();

    let mut client_config = ClientConfig::read()?;

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

        client_config.add_server(ClientServerConfig {
            name: "cloud".to_string(),
            server: Some(nebu_url),
            api_key: Some(api_key),
            auth_server: Some(auth_url),
        }, true);
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

        client_config.add_server(ClientServerConfig {
            name: "nebu".to_string(),
            server: Some(nebu_url),
            api_key: Some(api_key),
            auth_server: None,
        }, true);
    }
    client_config.write()?;

    // TODO: Check that we can actually reach and authenticate with the server

    println!("\nLogin successful!");
    Ok(())
}
