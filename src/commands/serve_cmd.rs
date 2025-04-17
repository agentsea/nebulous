use nebulous::create_app;
use nebulous::create_app_state;
use nebulous::proxy::server::start_proxy;
use nebulous::resources::v1::containers::controller::ContainerController;
use nebulous::resources::v1::processors::controller::ProcessorController;
use serde_json::Value;
use std::error::Error;
use std::io::Write;
use std::ops::Add;
use std::process::{Command, Stdio};
use nebulous::config::ClientConfig;

pub async fn launch_server(
    host: String,
    port: u16,
    disable_internal_auth: bool,
    auth_port: u16,
) -> Result<(), Box<dyn Error>> {
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

    if !disable_internal_auth {
        println!("Starting auth server");
        let mut config = ClientConfig::read()?;
        config.set_internal_auth_port(auth_port);
        config.write()?;

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

const BASE_COMPOSE: &str = include_str!("../../deploy/docker/docker-compose.yml");

pub async fn launch_docker(
    port: u16,
    disable_internal_auth: bool,
    auth_port: u16,
) -> Result<(), Box<dyn Error>> {
    Command::new("docker")
        .args(&["compose", "version"])
        .output()
        .expect(
            "Did not find docker compose. Please ensure that docker is installed and in your PATH.",
        );

    // Ask the user for the tailscale login server and auth key
    let tailscale_login_server = std::env::var("TS_LOGIN_SERVER").unwrap_or_else(|_| {
        println!("Please enter the Tailscale login server URL (or leave blank for default):");
        let mut input = String::new();
        std::io::stdin().read_line(&mut input).unwrap();
        input.trim().to_string()
    });
    let tailscale_auth_key = std::env::var("TS_AUTH_KEY").unwrap_or_else(|_| {
        println!("Please enter the Tailscale auth key:");
        let mut input = String::new();
        std::io::stdin().read_line(&mut input).unwrap();
        input.trim().to_string()
    });

    let mut doc: Value = serde_yaml::from_str(BASE_COMPOSE)?;

    doc["services"]["tailscale"]["environment"][2] =
        Value::String(format!("TS_AUTH_KEY={}", tailscale_auth_key));
    if !tailscale_login_server.is_empty() {
        doc["services"]["tailscale"]["environment"][3] = Value::String(format!(
            "TS_EXTRA_ARGS=--login-server {}",
            tailscale_login_server
        ));
    }

    let mut command = format!(
        "exec nebu serve --host 0.0.0.0 --port {} --auth-port {}",
        port, auth_port
    );
    if disable_internal_auth {
        command = command.add(" --disable-internal-auth");
    };
    doc["services"]["nebulous"]["command"][2] = Value::String(command);

    let yaml = serde_yaml::to_string(&doc)?;

    let mut child = Command::new("docker")
        .args(&["compose", "-f", "-", "up"])
        .stdin(Stdio::piped())
        .spawn()?;

    {
        let stdin = child.stdin.as_mut().expect("Failed to open stdin");
        stdin.write_all(yaml.as_bytes())?;
    }

    let status = child.wait()?;
    if !status.success() {
        Err("docker compose failed".into())
    } else {
        Ok(())
    }
}
