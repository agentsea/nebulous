use nebulous::create_app;
use nebulous::create_app_state;
use nebulous::proxy::server::start_proxy;
use nebulous::resources::v1::containers::controller::ContainerController;
use nebulous::resources::v1::processors::controller::ProcessorController;
use std::default::Default;
use std::error::Error;

pub async fn execute(
    host: String,
    port: u16,
    internal_auth: bool,
    auth_port: u16,
    docker: bool,
) -> Result<(), Box<dyn Error>> {
    if docker {
        println!("Starting Nebulous in Docker mode...");
        serve_docker_with_compose(host, port, internal_auth, auth_port).await?;
        return Ok(());
    }

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
        println!("Running docker compose down to clean up...");
        let status = std::process::Command::new("docker")
            .args(["compose", "-f", &self.compose_path, "down"])
            .status();
        
        match status {
            Ok(status) if status.success() => {
                println!("Docker stack cleaned up successfully.");
            }
            Ok(_) => {
                println!("Warning: docker compose down failed.");
            }
            Err(e) => {
                println!("Warning: Failed to run docker compose down: {}", e);
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
    println!("This will use docker compose to start the full stack with prebuilt images.");
    println!("Make sure you have Docker and docker compose installed.");
    println!("Press Ctrl+C to stop and clean up.");
    
    
    let (compose_path, _guard) = create_temp_compose_file()?;
    
    let (mut signal_handler, _shutdown_tx) = SignalHandler::new();
    let mut docker_manager = DockerComposeManager::new(compose_path.clone())?;
    
    let stdout = docker_manager.child.stdout.take().unwrap();
    let stderr = docker_manager.child.stderr.take().unwrap();
    let output_streamer = OutputStreamer::new(stdout, stderr);
    
    let result = tokio::select! {
        _ = signal_handler.wait_for_shutdown() => {
            println!("Shutting down docker compose...");
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
                    Err(format!("Failed to start docker compose: {}", e).into())
                }
            }
        }
    };
    
    result
}


fn create_temp_compose_file() -> Result<(String, TempFileGuard), Box<dyn Error>> {
    // Embed docker-compose file in binary
    const DOCKER_COMPOSE_CONTENT: &str = include_str!("../../deploy/docker/docker-compose.yaml");
    
    // Create a temporary docker compose file
    let temp_dir = std::env::temp_dir();
    let compose_path = temp_dir.join("docker-compose.yaml");
    
    // Write the embedded content to the temporary file
    std::fs::write(&compose_path, DOCKER_COMPOSE_CONTENT)?;
    
    let docker_compose_path = compose_path.to_str().unwrap().to_string();
    let guard = TempFileGuard(compose_path);
    
    Ok((docker_compose_path, guard))
}
