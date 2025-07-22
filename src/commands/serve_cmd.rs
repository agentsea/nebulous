// src/commands/serve.rs

use bollard::container::*;
use bollard::errors::Error as BollardError;
use bollard::models::*;
use bollard::volume::CreateVolumeOptions;
use bollard::Docker;
use futures_util::stream::TryStreamExt;
use nebulous::create_app;
use nebulous::create_app_state;
use nebulous::proxy::server::start_proxy;
use nebulous::resources::v1::containers::controller::ContainerController;
use nebulous::resources::v1::processors::controller::ProcessorController;
use std::collections::HashMap;
use std::default::Default;
use std::error::Error;
use tokio::time::{sleep, Duration};

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
        serve_docker(host, port, internal_auth, auth_port).await?;
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

async fn serve_docker(
    host: String,
    port: u16,
    internal_auth: bool,
    auth_port: u16,
) -> Result<(), Box<dyn Error>> {
    let docker = Docker::connect_with_local_defaults()?;

    // Ensure volumes exist
    for name in ["nebu-ts-authkey", "nebu-ts-sock", "nebu-tmp"] {
        docker
            .create_volume(CreateVolumeOptions {
                name,
                ..Default::default()
            })
            .await
            .ok();
    }

    // Ensure containers exist and are started
    let ts_authkey = format!(
        "TS_AUTHKEY={}",
        std::env::var("TS_AUTHKEY").unwrap_or_default()
    );
    let ts_extra_args = format!(
        "TS_EXTRA_ARGS=--login-server {}",
        std::env::var("TS_LOGIN_SERVER").unwrap_or_default()
    );
    ensure_container(
        &docker,
        "tailscale",
        bollard::container::Config {
            image: Some("tailscale/tailscale:latest"),
            hostname: Some("nebulous"),
            env: Some(vec![
                "TS_STATE_DIR=/var/lib/tailscale",
                "TS_USERSPACE=false",
                &ts_authkey,
                &ts_extra_args,
            ]),
            host_config: Some(HostConfig {
                cap_add: Some(vec!["NET_ADMIN".to_string()]),
                devices: Some(vec![DeviceMapping {
                    path_on_host: Some("/dev/net/tun".to_string()),
                    path_in_container: Some("/dev/net/tun".to_string()),
                    ..Default::default()
                }]),
                binds: Some(vec![
                    "nebu-ts-authkey:/var/lib/tailscale".to_string(),
                    "nebu-ts-sock:/var/run/tailscale".to_string(),
                    "nebu-tmp:/tmp".to_string(),
                ]),
                restart_policy: Some(RestartPolicy {
                    name: Some(RestartPolicyNameEnum::UNLESS_STOPPED),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        },
    )
    .await?;

    ensure_container(
        &docker,
        "postgres",
        bollard::container::Config {
            image: Some("postgres:17"),
            env: Some(vec!["POSTGRES_PASSWORD=postgres"]),
            exposed_ports: Some(HashMap::from([("5432/tcp", HashMap::new())])),
            host_config: Some(HostConfig {
                port_bindings: Some(HashMap::from([(
                    "5432/tcp".to_string(),
                    Some(vec![PortBinding {
                        host_ip: Some("0.0.0.0".to_string()),
                        host_port: Some("5432".to_string()),
                    }]),
                )])),
                restart_policy: Some(RestartPolicy {
                    name: Some(RestartPolicyNameEnum::UNLESS_STOPPED),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        },
    )
    .await?;

    ensure_container(
        &docker,
        "redis",
        bollard::container::Config {
            image: Some("redis:8"),
            host_config: Some(HostConfig {
                network_mode: Some("container:tailscale".to_string()),
                restart_policy: Some(RestartPolicy {
                    name: Some(RestartPolicyNameEnum::UNLESS_STOPPED),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        },
    )
    .await?;

    // Nebulous environment setup
    // TODO: Populate from config where applicable. App should parse complete config before launching.

    let rust_log = format!(
        "RUST_LOG={}",
        std::env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string())
    );
    let nebu_bucket_name = format!(
        "NEBU_BUCKET_NAME={}",
        std::env::var("NEBU_BUCKET_NAME").expect("NEBU_BUCKET_NAME must be set")
    );
    let nebu_bucket_region = format!(
        "NEBU_BUCKET_REGION={}",
        std::env::var("NEBU_BUCKET_REGION").unwrap_or_else(|_| "us-west-2".to_string())
    );
    let nebu_root_owner = format!(
        "NEBU_ROOT_OWNER={}",
        std::env::var("NEBU_ROOT_OWNER").unwrap_or_else(|_| "nebulous".to_string())
    );
    let nebu_publish_url = format!(
        "NEBU_PUBLISH_URL={}",
        std::env::var("NEBU_PUBLISH_URL").unwrap_or_else(|_| "http://localhost:3000".to_string())
    );
    let ts_apikey = format!(
        "TS_APIKEY={}",
        std::env::var("TS_APIKEY").unwrap_or_else(|_| "your-ts-apikey".to_string())
    );

    let envs = vec![
        "DATABASE_HOST=postgres",
        "DATABASE_PORT=5342",
        "DATABASE_USER=postgres",
        "DATABASE_PASSWORD=postgres",
        "DATABASE_URL=postgres://postgres:postgres@postgres:5432/postgres",
        "REDIS_HOST=localhost",
        "REDIS_PORT=6379",
        &ts_apikey,
        &rust_log,
        &nebu_bucket_name,
        &nebu_bucket_region,
        &nebu_root_owner,
        &nebu_publish_url,
    ];

    ensure_container(
        &docker,
        "nebu",
        bollard::container::Config {
            // TODO: Set to version from Cargo.toml
            image: Some("us-docker.pkg.dev/agentsea-dev/nebulous/server:latest"),
            env: Some(envs),
            cmd: Some(vec![
                "sh",
                "-c",
                "exec nebu serve --host 0.0.0.0 --port 3000",
            ]),
            host_config: Some(HostConfig {
                network_mode: Some("container:tailscale".to_string()),
                restart_policy: Some(RestartPolicy {
                    name: Some(RestartPolicyNameEnum::UNLESS_STOPPED),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        },
    )
    .await?;

    println!("All containers are up.");
    Ok(())
}

async fn ensure_container(
    docker: &Docker,
    name: &str,
    config: bollard::container::Config<&str>,
) -> Result<(), BollardError> {
    let containers = docker
        .list_containers(Some(ListContainersOptions::<String> {
            all: true,
            ..Default::default()
        }))
        .await?;

    let exists = containers.iter().any(|c| {
        c.names
            .as_ref()
            .map(|names| names.iter().any(|n| n.trim_start_matches('/') == name))
            .unwrap_or(false)
    });

    if !exists {
        println!("Creating container: {name}");
        docker
            .create_container(
                Some(CreateContainerOptions {
                    name,
                    ..Default::default()
                }),
                config,
            )
            .await?;
    } else {
        println!("Container '{name}' already exists.");
    }

    let details = docker.inspect_container(name, None).await?;
    let running = details
        .state
        .as_ref()
        .and_then(|s| s.running)
        .unwrap_or(false);

    if !running {
        println!("Starting container: {name}");
        docker
            .start_container(name, None::<StartContainerOptions<String>>)
            .await?;
    } else {
        println!("Container '{name}' is already running.");
    }

    Ok(())
}
