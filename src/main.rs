mod cli;
mod commands;
mod models;
use std::path::Path;

use crate::cli::{
    ApiKeyActions, AuthCommands, Cli, Commands, CreateCommands, DeleteCommands, GetCommands,
    ProxyCommands, SelectCommands, SendCommands, SetCommands, ShowCommands, SyncCommands,
};
use clap::Parser;
use nebulous::select::checkpoint::select_checkpoint;
use std::error::Error;
use tracing_subscriber;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    // Initialize logging with INFO level
    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "info");
    }
    // Initialize tracing
    tracing_subscriber::fmt::init();

    // Initialize VPN client
    if let Err(e) = nebulous::vpn::init_vpn_from_config().await {
        eprintln!("Failed to initialize VPN client: {}", e);
    }

    // Parse command-line arguments
    let cli = Cli::parse();

    match cli.command {
        Commands::Serve {
            host,
            port,
            internal_auth,
            auth_port,
        } => {
            commands::serve_cmd::execute(host, port, internal_auth, auth_port).await?;
        }
        Commands::Sync { command } => match command {
            SyncCommands::Volumes {
                config,
                interval_seconds,
                create_if_missing,
                watch,
                background,
                block_once,
                config_from_env,
            } => {
                commands::sync_cmd::execute_sync(
                    config,
                    interval_seconds,
                    create_if_missing,
                    watch,
                    background,
                    block_once,
                    config_from_env,
                )
                .await?;
            }
            SyncCommands::Wait {
                config,
                interval_seconds,
            } => {
                commands::sync_cmd::execute_wait(&config, interval_seconds).await?;
            }
        },
        Commands::Create { command } => match command {
            CreateCommands::Containers { command } => {
                // Add debug output to help diagnose the issue
                println!("Attempting to create container with command");

                // Wrap the call in a match to catch and print any errors
                match commands::create_cmd::create_container(command).await {
                    Ok(_) => (),
                    Err(e) => {
                        eprintln!("Error creating container: {:?}", e);
                        return Err(e);
                    }
                }
            }
            CreateCommands::Secrets { command } => {
                commands::create_cmd::create_secret(command).await?;
            }
        },
        Commands::Get { command } => match command {
            GetCommands::Accelerators { platform } => {
                commands::get_cmd::get_accelerators(platform).await?;
            }
            GetCommands::Containers { id } => {
                commands::get_cmd::get_containers(id).await?;
            }
            GetCommands::Platforms => {
                commands::get_cmd::get_platforms().await?;
            }
            GetCommands::Secrets { id } => {
                commands::get_cmd::get_secrets(id).await?;
            }
            GetCommands::Processors { name, namespace } => {
                commands::get_cmd::get_processors(name, namespace).await?;
            }
        },
        Commands::Delete { command } => match command {
            DeleteCommands::Containers {
                name,
                namespace,
                all,
            } => {
                commands::delete_cmd::delete_container(name, namespace, all).await?;
            }
            DeleteCommands::Processors { name, namespace } => {
                commands::delete_cmd::delete_processor(name, namespace).await?;
            }
        },
        Commands::Proxy { command } => match command {
            ProxyCommands::Shell { host, port } => {
                commands::proxy_cmd::run_sync_cmd_server(&host, port).await?;
            }
        },
        Commands::Select { command } => match command {
            SelectCommands::Checkpoint { base_dir, criteria } => {
                match select_checkpoint(Path::new(&base_dir), &criteria) {
                    Ok(Some(checkpoint)) => println!("{}", checkpoint.to_str().unwrap_or("")),
                    Ok(None) => println!("No checkpoint found"),
                    Err(e) => {
                        eprintln!("Error selecting checkpoint: {:?}", e);
                    }
                }
            }
        },
        Commands::Send { command } => match command {
            SendCommands::Messages { command } => {
                commands::send_cmd::send_messages(&command).await?;
            }
        },
        Commands::Daemon {
            host,
            port,
            background,
        } => {
            commands::daemon_cmd::execute_daemon(&host, port, background).await?;
        }
        Commands::Logs {
            name,
            namespace,
            follow,
        } => {
            commands::log_cmd::fetch_container_logs(name, namespace, follow).await?;
        }
        Commands::Login { url, auth, hub } => {
            commands::login_cmd::execute(url, auth, hub).await?;
        }
        Commands::Exec(args) => {
            commands::exec_cmd::exec_cmd(args).await?;
        }
        Commands::Auth { command } => match command {
            AuthCommands::ApiKeys { action } => match action {
                ApiKeyActions::List => {
                    commands::auth_cmd::list_api_keys().await?;
                }
                ApiKeyActions::Get { id } => {
                    commands::auth_cmd::get_api_key(&id).await?;
                }
                ApiKeyActions::Generate => {
                    commands::auth_cmd::generate_api_key().await?;
                }
                ApiKeyActions::Revoke { id } => {
                    commands::auth_cmd::revoke_api_key(&id).await?;
                }
            },
        },
        Commands::Show { command } => match command {
            ShowCommands::Config => {
                commands::show_cmd::show_config().await?;
            }
        },
        Commands::Set { command } => match command {
            SetCommands::Context { name } => {
                commands::set_cmd::set_context(&name).await?;
            }
        },
    }

    Ok(())
}
