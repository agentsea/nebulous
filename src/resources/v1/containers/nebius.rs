use crate::entities::containers;
use crate::models::V1UserProfile;
use crate::mutation::Mutation;
use crate::query::Query;
use crate::resources::v1::containers::base::{ContainerPlatform, ContainerStatus};
use crate::resources::v1::containers::models::{
    V1Container, V1ContainerRequest, V1ContainerStatus,
};
use crate::ssh::exec::run_ssh_command_ts;

use nebius::nebius::common::v1::Operation;
use nebius::nebius::compute::v1::{
    attached_disk_spec, instance_service_client::InstanceServiceClient, resources_spec,
    AttachedDiskSpec, CreateInstanceRequest, DeleteInstanceRequest, NetworkInterfaceSpec,
    ResourcesSpec,
};
use petname;
use sea_orm::{ActiveModelTrait, DatabaseConnection, Set};
use short_uuid::ShortUuid;
use std::collections::HashMap;
use tonic::transport::Channel;
use tonic::Request;
use tracing::{debug, error, info};

#[derive(Clone)]
pub struct NebiusPlatform {
    compute_client: InstanceServiceClient<Channel>,
    token: String,
    http_client: reqwest::Client,
}

impl NebiusPlatform {
    pub async fn new() -> Self {
        let endpoint = "https://compute.api.nebius.com:443";
        let token = std::env::var("NEBIUS_API_KEY")
            .expect("[Nebius Controller] Missing NEBIUS_API_KEY environment variable");

        let channel = Channel::from_static(endpoint)
            .connect()
            .await
            .expect("Failed to connect to Nebius API");

        NebiusPlatform {
            compute_client: InstanceServiceClient::new(channel),
            token,
            http_client: reqwest::Client::new(),
        }
    }

    fn build_node_user_data(&self, ts_auth_key: &str, hostname: &str) -> String {
        // This script is for Debian/Ubuntu based images
        format!(
            r#"#!/bin/bash
        apt-get update
        apt-get install -y docker.io curl
        
        curl -fsSL https://tailscale.com/install.sh | sh
        
        tailscaled --tun=userspace-networking &
        
        tailscale up --auth-key={} --hostname={} --ssh --advertise-tags=tag:node
        "#,
            ts_auth_key, hostname
        )
    }

    fn create_request<T>(&self, message: T) -> Request<T> {
        let mut req = Request::new(message);
        req.metadata_mut().insert(
            "authorization",
            format!("Bearer {}", self.token).parse().unwrap(),
        );
        req
    }

    async fn create(
        &self,
        db: &DatabaseConnection,
        model: &containers::Model,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!("[Nebius] Creating container resource: {}", model.id);

        Mutation::update_container_status(
            db,
            model.id.clone(),
            Some(ContainerStatus::Creating.to_string()),
            Some("Provisioning VM on Nebius".to_string()),
            None,
            None,
            None,
            None,
            None,
        )
        .await?;

        let folder_id = std::env::var("NEBIUS_FOLDER_ID").expect("NEBIUS_FOLDER_ID not set");
        let subnet_id = std::env::var("NEBIUS_SUBNET_ID").expect("NEBIUS_SUBNET_ID not set");

        let hostname = self.get_tailscale_device_name(model).await;
        let ts_auth_key = self.get_tailscale_device_key(model).await?;
        let user_data = self.build_node_user_data(&ts_auth_key, &hostname);

        let request = CreateInstanceRequest {
            folder_id,
            name: hostname,
            resources_spec: Some(ResourcesSpec {
                memory: 4 * 1024 * 1024 * 1024, // 4GB
                cores: 2,
                ..Default::default()
            }),
            zone_id: "ru-central1-a".to_string(),
            platform_id: "standard-v3".to_string(),
            boot_disk_spec: Some(AttachedDiskSpec {
                auto_delete: true,
                disk: Some(attached_disk_spec::Disk::DiskSpec(
                    attached_disk_spec::DiskSpec {
                        image_id: "fd8v95d8o2285pif4go1".to_string(), // Ubuntu 22.04 LTS
                        size: 20 * 1024 * 1024 * 1024,                // 20GB
                        ..Default::default()
                    },
                )),
                ..Default::default()
            }),
            network_interface_specs: vec![NetworkInterfaceSpec {
                subnet_id,
                ..Default::default()
            }],
            metadata: HashMap::from([("user-data".to_string(), user_data)]),
            ..Default::default()
        };

        let mut client = self.compute_client.clone();
        let response = client
            .create(self.create_request(request))
            .await?
            .into_inner();

        let operation_id = response.id;
        info!(
            "[Nebius] Instance creation started. Operation ID: {}",
            operation_id
        );

        Mutation::update_container_resource_name(db, model.id.clone(), operation_id).await?;

        Ok(())
    }

    async fn deploy_container_to_instance(
        &self,
        model: &containers::Model,
        db: &DatabaseConnection,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!("[Nebius] Deploying container {} to instance", model.id);
        let hostname = self.get_tailscale_device_name(model).await;
        let common_env = self.get_common_env(model, db).await?;

        let env_flags: String = common_env
            .iter()
            .map(|(k, v)| format!("-e {}='{}'", k, v))
            .collect::<Vec<String>>()
            .join(" ");

        let docker_command = format!(
            "docker run -d --name {} {} --restart unless-stopped {} {}",
            model.id,
            env_flags,
            model.image,
            model.command.as_deref().unwrap_or("")
        );

        let command_parts = docker_command
            .split_whitespace()
            .map(|s| s.to_string())
            .collect();

        // Nebius default user is 'nebius' for Ubuntu images
        run_ssh_command_ts(&hostname, command_parts, false, false, Some("nebius"))?;

        Ok(())
    }

    async fn watch(
        &self,
        db: &DatabaseConnection,
        container: &containers::Model,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!("[Nebius] Watching container {}", container.id);
        let operation_id = container
            .resource_name
            .as_ref()
            .ok_or("Container is missing operation ID to watch")?;

        let endpoint = format!(
            "https://operation.api.nebius.com/v1/operations/{}",
            operation_id
        );

        loop {
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;

            let response = self
                .http_client
                .get(&endpoint)
                .bearer_auth(&self.token)
                .send()
                .await?;

            if !response.status().is_success() {
                let error_text = response.text().await?;
                error!(
                    "[Nebius Watcher] Error polling operation {}: {}",
                    operation_id, error_text
                );
                continue;
            }

            let operation: Operation = response.json().await?;

            if operation.done {
                if let Some(error) = operation.error {
                    error!(
                        "[Nebius Watcher] VM creation failed for op {}: {:?}",
                        operation_id, error
                    );
                    Mutation::update_container_status(
                        db,
                        container.id.clone(),
                        Some(ContainerStatus::Failed.to_string()),
                        Some(format!("{:?}", error)),
                        None,
                        None,
                        None,
                        None,
                        None,
                    )
                    .await?;
                    return Err(format!("Nebius VM creation failed: {:?}", error).into());
                } else {
                    info!(
                        "[Nebius Watcher] VM for container {} is ready. Deploying container...",
                        container.id
                    );
                    self.deploy_container_to_instance(container, db).await?;
                    Mutation::update_container_status(
                        db,
                        container.id.clone(),
                        Some(ContainerStatus::Running.to_string()),
                        Some("Container running on Nebius VM".to_string()),
                        None,
                        None,
                        None,
                        None,
                        Some(true),
                    )
                    .await?;
                    return Ok(());
                }
            }
        }
    }

    async fn delete(
        &self,
        id: &str,
        db: &DatabaseConnection,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!("[Nebius] Deleting container {}", id);

        let container = Query::find_container_by_id(db, id.to_string())
            .await?
            .ok_or_else(|| format!("Container {} not found", id))?;

        if let Some(instance_id) = container.resource_name.as_ref() {
            let mut client = self.compute_client.clone();
            let request = DeleteInstanceRequest {
                instance_id: instance_id.to_string(),
            };

            let operation = client
                .delete(self.create_request(request))
                .await?
                .into_inner();

            info!(
                "[Nebius] Instance deletion started. Operation ID: {}",
                operation.id
            );
        } else {
            info!(
                "[Nebius] No instance_id found for container {}. Skipping Nebius deletion.",
                id
            );
        }

        Mutation::delete_container(db, id.to_string()).await?;

        info!("[Nebius] Container {} deleted from database.", id);

        Ok(())
    }
}

impl ContainerPlatform for NebiusPlatform {
    async fn declare(
        &self,
        config: &V1ContainerRequest,
        db: &DatabaseConnection,
        user_profile: &V1UserProfile,
        owner_id: &str,
        namespace: &str,
        api_key: Option<String>,
    ) -> Result<V1Container, Box<dyn std::error::Error + Send + Sync>> {
        let name = config
            .metadata
            .as_ref()
            .and_then(|meta| meta.name.clone())
            .unwrap_or_else(|| petname::petname(3, "-").unwrap());

        info!("[Nebius] Declaring container with name: {}", name);

        let id = ShortUuid::generate().to_string();

        self.store_agent_key_secret(db, user_profile, &id, owner_id, api_key)
            .await?;

        let owner_ref: Option<String> = config
            .metadata
            .as_ref()
            .and_then(|meta| meta.owner_ref.clone());

        let container = containers::ActiveModel {
            id: Set(id.clone()),
            namespace: Set(namespace.to_string()),
            name: Set(name.clone()),
            full_name: Set(format!("{}/{}", namespace, name)),
            owner: Set(owner_id.to_string()),
            owner_ref: Set(owner_ref.clone()),
            image: Set(config.image.clone()),
            env: Set(config.env.clone().map(|vars| serde_json::json!(vars))),
            volumes: Set(config.volumes.clone().map(|vols| serde_json::json!(vols))),
            platform: Set(Some("nebius".to_string())),
            status: Set(Some(serde_json::json!(V1ContainerStatus {
                status: Some(ContainerStatus::Defined.to_string()),
                message: None,
                accelerator: None, // TODO: Map from config.accelerators
                public_ports: None,
                cost_per_hr: None,
                tailnet_url: None,
                ready: None,
            }))),
            desired_status: Set(Some(ContainerStatus::Running.to_string())),
            ..Default::default()
        };

        let container_model = container.insert(db).await?;

        Ok(container_model.to_v1_container()?)
    }

    async fn reconcile(
        &self,
        container: &containers::Model,
        db: &DatabaseConnection,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!("[Nebius] Reconciling container {}", container.id);

        if let Ok(Some(status)) = container.parse_status() {
            let status_enum = status
                .status
                .and_then(|s| s.parse::<ContainerStatus>().ok())
                .unwrap_or(ContainerStatus::Invalid);

            if status_enum.needs_start() {
                self.create(db, container).await?;
            }

            if status_enum.needs_watch() {
                self.watch(db, container).await?;
            }
        }

        Ok(())
    }

    async fn exec(
        &self,
        _container_id: &str,
        _command: &str,
        _db: &DatabaseConnection,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        info!("[Nebius] Executing command in container");
        todo!()
    }

    async fn logs(
        &self,
        _container_id: &str,
        _db: &DatabaseConnection,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        info!("[Nebius] Getting container logs");
        todo!()
    }

    async fn delete(
        &self,
        id: &str,
        db: &DatabaseConnection,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!("[Nebius] Deleting container {}", id);

        let container = Query::find_container_by_id(db, id.to_string())
            .await?
            .ok_or_else(|| format!("Container {} not found", id))?;

        if let Some(instance_id) = container.resource_name.as_ref() {
            let mut client = self.compute_client.clone();
            let request = DeleteInstanceRequest {
                instance_id: instance_id.to_string(),
            };

            let operation = client
                .delete(self.create_request(request))
                .await?
                .into_inner();

            info!(
                "[Nebius] Instance deletion started. Operation ID: {}",
                operation.id
            );
        } else {
            info!(
                "[Nebius] No instance_id found for container {}. Skipping Nebius deletion.",
                id
            );
        }

        Mutation::delete_container(db, id.to_string()).await?;

        info!("[Nebius] Container {} deleted from database.", id);

        Ok(())
    }

    fn accelerator_map(&self) -> HashMap<String, String> {
        // TODO: Populate with actual Nebius instance types
        HashMap::new()
    }
}
