use bollard::container::*;
use bollard::errors::Error as BollardError;
use bollard::models::*;
use bollard::volume::CreateVolumeOptions;
use bollard::Docker;
use nebulous::create_app;
use nebulous::create_app_state;
use nebulous::proxy::server::start_proxy;
use nebulous::resources::v1::containers::controller::ContainerController;
use nebulous::resources::v1::processors::controller::ProcessorController;
use std::collections::HashMap;
use std::default::Default;
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
        // serve_docker_with_bollard(host, port, internal_auth, auth_port).await?;
        serve_docker_with_compose(host, port, internal_auth, auth_port).await?;
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

async fn serve_docker_with_bollard(
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
    println!("Test 1");
    ensure_container(
        &docker,
        "tailscale",
        bollard::container::Config {
            image: Some("tailscale/tailscale:stable"),
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
    println!("Tailscale container created");

    println!("Creating postgres container");
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
    println!("Postgres container created");

    println!("Creating redis container");
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
    println!("Redis container created");

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
        std::env::var("NEBU_BUCKET_REGION").expect("NEBU_BUCKET_REGION must be set")
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
            image: Some("us-docker.pkg.dev/agentsea-dev/nebulous/server:c2e0e00-binary-only"),
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
        println!("Test 2");
    } else {
        println!("Container '{name}' is already running.");
    }

    Ok(())
}

// RAII guard for temporary file cleanup
struct TempFileGuard(std::path::PathBuf);
impl Drop for TempFileGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

struct DockerComposeManager {
    child: std::process::Child,
    compose_path: String,
}

impl DockerComposeManager {
    fn new(compose_path: String) -> Result<Self, Box<dyn Error>> {

        use nebulous::config::SERVER_CONFIG;

        let child = std::process::Command::new("docker")
            .args(["compose", "-f", &compose_path, "up", "--build"])
            .env("NEBU_BUCKET_NAME", SERVER_CONFIG.bucket_name.clone())
            .env("NEBU_BUCKET_REGION", SERVER_CONFIG.bucket_region.clone())
            .env("NEBU_ROOT_OWNER", SERVER_CONFIG.root_owner.clone())
            .env("NEBU_PUBLISH_URL", SERVER_CONFIG.publish_url.clone().unwrap_or_default())
            .env("TS_APIKEY", SERVER_CONFIG.vpn.api_key.clone().unwrap_or_default())
            .env("RUST_LOG", "info")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()?;
        
        Ok(Self { child, compose_path })
    }
    
    fn kill(&mut self) {
        let _ = self.child.kill();
    }
    
    async fn wait(&mut self) -> Result<std::process::ExitStatus, std::io::Error> {
        self.child.wait()
    }
    
    async fn cleanup(&self) {
        println!("Running docker-compose down to clean up...");
        let status = std::process::Command::new("docker")
            .args(["compose", "-f", &self.compose_path, "down"])
            .status();
        
        match status {
            Ok(status) if status.success() => {
                println!("Docker stack cleaned up successfully.");
            }
            Ok(_) => {
                println!("Warning: docker-compose down failed.");
            }
            Err(e) => {
                println!("Warning: Failed to run docker-compose down: {}", e);
            }
        }
    }
}

struct OutputStreamer {
    stdout_handle: tokio::task::JoinHandle<()>,
    stderr_handle: tokio::task::JoinHandle<()>,
}

impl OutputStreamer {
    fn new(stdout: std::process::ChildStdout, stderr: std::process::ChildStderr) -> Self {
        let stdout_handle = tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            let mut reader = tokio::io::BufReader::new(
                tokio::process::ChildStdout::from_std(stdout).unwrap()
            );
            let mut buffer = [0; 1024];
            loop {
                match reader.read(&mut buffer).await {
                    Ok(0) => break, // EOF
                    Ok(n) => {
                        print!("{}", String::from_utf8_lossy(&buffer[..n]));
                    }
                    Err(_) => break,
                }
            }
        });
        
        let stderr_handle = tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            let mut reader = tokio::io::BufReader::new(
                tokio::process::ChildStderr::from_std(stderr).unwrap()
            );
            let mut buffer = [0; 1024];
            loop {
                match reader.read(&mut buffer).await {
                    Ok(0) => break, // EOF
                    Ok(n) => {
                        eprint!("{}", String::from_utf8_lossy(&buffer[..n]));
                    }
                    Err(_) => break,
                }
            }
        });
        
        Self { stdout_handle, stderr_handle }
    }
    
    async fn wait_for_completion(self) {
        let _ = self.stdout_handle.await;
        let _ = self.stderr_handle.await;
    }
}

struct SignalHandler {
    shutdown_rx: tokio::sync::broadcast::Receiver<()>,
}

impl SignalHandler {
    fn new() -> (Self, tokio::sync::broadcast::Sender<()>) {
        let (shutdown_tx, shutdown_rx) = tokio::sync::broadcast::channel::<()>(1);
        
        // Spawn signal handler
        let shutdown_tx_clone = shutdown_tx.clone();
        tokio::spawn(async move {
            tokio::signal::ctrl_c().await.unwrap();
            println!("\nReceived interrupt signal. Cleaning up...");
            let _ = shutdown_tx_clone.send(());
        });
        
        (Self { shutdown_rx }, shutdown_tx)
    }
    
    async fn wait_for_shutdown(&mut self) {
        let _ = self.shutdown_rx.recv().await;
    }
}

async fn serve_docker_with_compose(
    host: String,
    port: u16,
    _internal_auth: bool,
    _auth_port: u16,
) -> Result<(), Box<dyn Error>> {
    println!("Starting Nebulous in Docker mode...");
    println!("This will use docker-compose to start the full stack with prebuilt images.");
    println!("Make sure you have Docker and docker-compose installed.");
    println!("Press Ctrl+C to stop and clean up.");
    
    setup_environment()?;
    
    let (compose_path, _guard) = create_temp_compose_file()?;
    
    let (mut signal_handler, _shutdown_tx) = SignalHandler::new();
    let mut docker_manager = DockerComposeManager::new(compose_path.clone())?;
    
    let stdout = docker_manager.child.stdout.take().unwrap();
    let stderr = docker_manager.child.stderr.take().unwrap();
    let output_streamer = OutputStreamer::new(stdout, stderr);
    
    let result = tokio::select! {
        _ = signal_handler.wait_for_shutdown() => {
            println!("Shutting down docker-compose...");
            docker_manager.kill();
            tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
            docker_manager.cleanup().await;
            Ok(())
        }
        exit_status = async {
            output_streamer.wait_for_completion().await;
            docker_manager.wait().await
        } => {
            match exit_status {
                Ok(status) if status.success() => {
                    println!("Docker stack started successfully!");
                    println!("Nebulous server should be available at http://{}:{}", host, port);
                    Ok(())
                }
                Ok(_) => {
                    Err("Docker-compose failed to start properly.".into())
                }
                Err(e) => {
                    Err(format!("Failed to start docker-compose: {}", e).into())
                }
            }
        }
    };
    
    result
}

fn setup_environment() -> Result<(), Box<dyn Error>> {
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
    
    // Set the version for the prebuilt image
    let version = env!("CARGO_PKG_VERSION");
    std::env::set_var("NEBU_VERSION", version);
    println!("Using Nebulous version: {}", version);
    
    Ok(())
}

fn create_temp_compose_file() -> Result<(String, TempFileGuard), Box<dyn Error>> {
    // Embed docker-compose file in binary
    const DOCKER_COMPOSE_CONTENT: &str = include_str!("../../deploy/docker/docker-compose.yaml");
    
    // Create a temporary docker-compose file
    let temp_dir = std::env::temp_dir();
    let compose_path = temp_dir.join("docker-compose.yaml");
    
    // Write the embedded content to the temporary file
    std::fs::write(&compose_path, DOCKER_COMPOSE_CONTENT)?;
    
    let docker_compose_path = compose_path.to_str().unwrap().to_string();
    let guard = TempFileGuard(compose_path);
    
    Ok((docker_compose_path, guard))
}
