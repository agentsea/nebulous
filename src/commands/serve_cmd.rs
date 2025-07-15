// src/commands/serve.rs

use nebulous::create_app;
use nebulous::create_app_state;
use nebulous::proxy::server::start_proxy;
use nebulous::resources::v1::containers::controller::ContainerController;
use nebulous::resources::v1::processors::controller::ProcessorController;
use std::error::Error;

pub async fn execute(
    host: String,
    port: u16,
    internal_auth: bool,
    auth_port: u16,
    docker: bool,
) -> Result<(), Box<dyn Error>> {
    // Check docker flag FIRST, before any configuration validation
    if docker {
        println!("Starting Nebulous in Docker mode...");
        println!("This will use docker-compose to start the full stack.");
        println!("Make sure you have Docker and docker-compose installed.");
        
        // Set default environment variables for docker mode to prevent validation errors
        if std::env::var("NEBU_BUCKET_NAME").is_err() {
            std::env::set_var("NEBU_BUCKET_NAME", "nebulous");
        }
        if std::env::var("NEBU_BUCKET_REGION").is_err() {
            std::env::set_var("NEBU_BUCKET_REGION", "us-east-1");
        }
        if std::env::var("NEBU_ROOT_OWNER").is_err() {
            std::env::set_var("NEBU_ROOT_OWNER", "me");
        }
        
        if !std::path::Path::new("docker-compose.yml").exists() {
            return Err("docker-compose.yml not found in current directory. Please run this command from the project root.".into());
        }
        
        let status = std::process::Command::new("docker compose")
            .args(["up", "--build"])
            .status()?;
            
        if !status.success() {
            return Err("Failed to start docker-compose. Check your Docker installation and docker-compose.yml file.".into());
        }
        
        println!("Docker stack started successfully!");
        println!("Nebulous server should be available at http://{}:{}", host, port);
        return Ok(());
    }

    // Only validate configuration if NOT running in docker mode
    let app_state = create_app_state().await?;
    let app = create_app(app_state.clone()).await;

    println!("Starting container controller");
    let controller = ContainerController::new(std::sync::Arc::new(app_state.clone()));
    controller.spawn_reconciler();
    println!("Container controller started");

    println!("Starting processor controller");
    let processor_controller = ProcessorController::new(std::sync::Arc::new(app_state.clone()));
    processor_controller.spawn_reconciler();
    println!("Processor controller started");

    println!("Starting proxy server");
    tokio::spawn({
        let proxy_state = app_state.clone();
        async move {
            if let Err(e) = start_proxy(proxy_state, 3030).await {
                eprintln!("Error in proxy server: {}", e);
            }
        }
    });
    println!("Proxy server started in background");

    if internal_auth {
        println!("Starting auth server");
        tokio::spawn({
            let auth_state = app_state.clone();
            async move {
                if let Err(e) =
                    nebulous::auth::server::main::start_auth_server(auth_state, auth_port).await
                {
                    eprintln!("Error in auth server: {}", e);
                }
            }
        });
        println!("Auth server started in background");
    } else {
        println!("Launching without internal auth server.");
    }

    // Run it
    println!("Starting main server");
    let addr = format!("{}:{}", host, port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    println!("Server running at http://{}", addr);
    axum::serve(listener, app).await?;

    Ok(())
}
