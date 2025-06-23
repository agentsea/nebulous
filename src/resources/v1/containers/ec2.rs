use crate::entities::containers;
use crate::models::V1UserProfile;
use crate::mutation::Mutation;
use crate::query::Query;
use crate::resources::v1::containers::base::{ContainerPlatform, ContainerStatus};
use crate::resources::v1::containers::models::{
    V1Container, V1ContainerHealthCheck, V1ContainerRequest, V1ContainerStatus,
};
use crate::ssh::exec::run_ssh_command_ts;
use aws_config::SdkConfig;
use aws_sdk_ec2::{
    client::Waiters,
    types::{Filter, InstanceStateName, InstanceType, ResourceType, Tag, TagSpecification},
    Client as Ec2Client,
};
use base64::{engine::general_purpose, Engine as _};
use petname;
use sea_orm::{ActiveModelTrait, DatabaseConnection, Set};
use short_uuid::ShortUuid;
use std::collections::HashMap;
use std::str::FromStr;
use tracing::{debug, error, info};

#[derive(Clone)]
pub struct Ec2Platform {
    ec2_client: Ec2Client,
    sdk_config: SdkConfig,
}

impl Ec2Platform {
    pub async fn new() -> Self {
        let sdk_config = aws_config::load_from_env().await;
        Ec2Platform {
            ec2_client: Ec2Client::new(&sdk_config),
            sdk_config,
        }
    }

    fn build_node_user_data(&self, ts_auth_key: &str, hostname: &str) -> String {
        let log_file = "/var/log/nebu_node_setup.log";
        // This script just sets up the machine as a node, but doesn't run a specific container
        format!(
            r#"#!/bin/bash
    exec > >(tee -a {log_file}) 2>&1
    
    echo "[DEBUG] Starting node setup..."
    yum update -y
    yum install -y docker
    service docker start
    usermod -a -G docker ec2-user
    
    echo "[DEBUG] Installing curl (if not present)..."
    if ! command -v curl &> /dev/null; then
        yum install -y curl
    fi
    
    echo "[DEBUG] Installing tailscale (if not present)..."
    if ! command -v tailscale &> /dev/null; then
        echo "[DEBUG] Tailscale not installed. Installing..."
        curl -fsSL https://tailscale.com/install.sh | sh
    else
        echo "[DEBUG] Tailscale already installed."
    fi
    
    echo "[DEBUG] Starting tailscale daemon ..."
    tailscaled --tun=userspace-networking --socks5-server=localhost:1055 --outbound-http-proxy-listen=localhost:1055 > /var/log/tailscaled.log 2>&1 &

    sleep 5

    echo "[DEBUG] Starting tailscale up..."
    tailscale up --auth-key={ts_auth_key} --hostname="{hostname}" --ssh --advertise-tags=tag:node

    echo "[DEBUG] Node setup complete."
    "#,
            log_file = log_file,
            ts_auth_key = ts_auth_key,
            hostname = hostname
        )
    }

    async fn find_suitable_instance(
        &self,
        model: &containers::Model,
        db: &DatabaseConnection,
    ) -> Result<Option<String>, Box<dyn std::error::Error + Send + Sync>> {
        // 1. Get container resource requirements from model.
        let requested_resources = model.parse_resources()?.unwrap_or_default();
        let requested_cpu = requested_resources.min_cpu.unwrap_or(0.1); // default small request
        let requested_memory = requested_resources.min_memory.unwrap_or(0.256); // default 256MB

        // 2. Describe EC2 instances with a specific tag.
        let filter_tag = Filter::builder()
            .name("tag:nebulous-node")
            .values("true")
            .build();

        let filter_state = Filter::builder()
            .name("instance-state-name")
            .values(InstanceStateName::Running.as_ref())
            .build();

        let describe_output = self
            .ec2_client
            .describe_instances()
            .filters(filter_tag)
            .filters(filter_state)
            .send()
            .await?;

        // 3. For each instance...
        for reservation in describe_output.reservations() {
            for instance in reservation.instances() {
                let instance_id = if let Some(id) = instance.instance_id() {
                    id.to_string()
                } else {
                    continue;
                };

                let instance_type = if let Some(it) = instance.instance_type() {
                    it
                } else {
                    continue;
                };

                // a. Get its total capacity
                let (total_cpu, total_memory) = get_instance_capacity(instance_type);

                // b. Find containers on it
                let containers_on_instance =
                    Query::find_containers_by_resource_name(db, &instance_id).await?;

                // c. Sum up used resources
                let mut used_cpu = 0.0;
                let mut used_memory = 0.0;
                for container in containers_on_instance {
                    if let Some(res) = container.parse_resources()? {
                        used_cpu += res.min_cpu.unwrap_or(0.0);
                        used_memory += res.min_memory.unwrap_or(0.0);
                    }
                }

                // d. Check if there's enough space
                let available_cpu = total_cpu - used_cpu;
                let available_memory = total_memory - used_memory;

                info!(
                    "[Ec2 Scheduler] Instance {}: Available CPU={} | Available Memory={}",
                    instance_id, available_cpu, available_memory
                );

                if available_cpu >= requested_cpu && available_memory >= requested_memory {
                    info!("[Ec2 Scheduler] Found suitable instance {}", instance_id);
                    return Ok(Some(instance_id));
                }
            }
        }

        info!("[Ec2 Scheduler] No suitable instance found.");
        Ok(None)
    }

    async fn provision_new_instance(
        &self,
        model: &containers::Model,
        _db: &DatabaseConnection,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        info!(
            "[Ec2 Controller] Provisioning new EC2 instance for container {}",
            model.id
        );

        // 1. Choose instance type from container spec
        let instance_type_str = model
            .status
            .as_ref()
            .and_then(|s| s.as_object())
            .and_then(|s| s.get("accelerator"))
            .and_then(|a| a.as_str())
            .unwrap_or("t2.micro");
        let instance_type = InstanceType::from(instance_type_str);

        // 2. Find a suitable AMI (e.g., latest Amazon Linux 2)
        // TODO: Find this dynamically
        let ami_id = "ami-0c55b159cbfafe1f0"; // Amazon Linux 2 AMI (HVM), SSD Volume Type

        // 3. Find or create a security group
        // TODO: Create/find this dynamically, ensure port 22 is open for SSH
        let security_group_id = "sg-0123456789abcdef0"; // Placeholder

        // 4. Create user-data script
        let hostname = self.get_tailscale_device_name(model).await;
        let ts_auth_key = self.get_tailscale_device_key(model).await?;
        let user_data_script = self.build_node_user_data(&ts_auth_key, &hostname);
        let user_data_base64 = general_purpose::STANDARD.encode(user_data_script);

        // 5. Tag the instance
        let tag_spec = TagSpecification::builder()
            .resource_type(ResourceType::Instance)
            .tags(Tag::builder().key("Name").value(hostname.clone()).build())
            .tags(Tag::builder().key("nebulous-node").value("true").build())
            .tags(
                Tag::builder()
                    .key("owner")
                    .value(model.owner.clone())
                    .build(),
            )
            .build();

        // 6. Launch instance
        let run_instances_output = self
            .ec2_client
            .run_instances()
            .image_id(ami_id)
            .instance_type(instance_type)
            .min_count(1)
            .max_count(1)
            .security_group_ids(security_group_id)
            .user_data(user_data_base64)
            .tag_specifications(tag_spec)
            .send()
            .await?;

        let instance_id = run_instances_output
            .instances
            .ok_or("No instances returned from EC2")?
            .get(0)
            .ok_or("No instance in instance list")?
            .instance_id
            .as_deref()
            .ok_or("Instance has no ID")?
            .to_string();

        info!(
            "[Ec2 Controller] Launched new instance {} for nebulous pool",
            instance_id
        );

        // 7. Wait for the instance to be running
        info!(
            "[Ec2 Controller] Waiting for instance {} to be running...",
            instance_id
        );
        self.ec2_client
            .wait_until_instance_running()
            .instance_ids(instance_id.clone())
            .wait(std::time::Duration::from_secs(600))
            .await?;

        info!("[Ec2 Controller] Instance {} is running.", instance_id);

        // It takes a bit for ssh to be available after starting.
        tokio::time::sleep(std::time::Duration::from_secs(20)).await;

        Ok(instance_id)
    }

    async fn deploy_container_to_instance(
        &self,
        instance_id: &str,
        model: &containers::Model,
        db: &DatabaseConnection,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!(
            "[Ec2 Controller] Deploying container {} to instance {}",
            model.id, instance_id
        );

        // 1. Get common environment variables
        let common_env = self.get_common_env(model, db).await?;
        let env_flags: String = common_env
            .iter()
            .map(|(k, v)| format!("-e {}='{}'", k, v))
            .collect::<Vec<String>>()
            .join(" ");

        // TODO: This needs to handle volumes properly
        let volume_flags = "";

        // 2. Construct `docker run` command
        let docker_command = format!(
            "docker run -d --name {} {} {} {} {} {}",
            model.id,
            env_flags,
            volume_flags,
            "--restart unless-stopped", // Example restart policy
            model.image,
            model.command.as_deref().unwrap_or("")
        );

        // 3. Get instance hostname for SSH
        let hostname = self.get_tailscale_device_name(model).await;
        info!(
            "[Ec2 Controller] SSHing to {} to run command: {}",
            hostname, docker_command
        );

        let command_parts: Vec<String> = docker_command
            .split_whitespace()
            .map(|s| s.to_string())
            .collect();
        run_ssh_command_ts(&hostname, command_parts, false, false, Some("ec2-user"))?;

        // 4. Update container status
        Mutation::update_container_status(
            db,
            model.id.clone(),
            Some(ContainerStatus::Running.to_string()),
            Some(format!("Running on instance {}", instance_id)),
            None,
            None,
            None,
            None,
            None,
        )
        .await?;

        info!(
            "[Ec2 Controller] Successfully deployed container {} to instance {}",
            model.id, instance_id
        );

        Ok(())
    }

    async fn create(
        &self,
        db: &DatabaseConnection,
        model: containers::Model,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        Mutation::update_container_status(
            db,
            model.id.clone(),
            Some(ContainerStatus::Creating.to_string()),
            Some("Finding available resources".to_string()),
            None,
            None,
            None,
            None,
            None,
        )
        .await?;

        // 1. Try to find a suitable existing instance
        let instance_id = match self.find_suitable_instance(&model, db).await? {
            Some(id) => {
                info!(
                    "[Ec2 Controller] Found suitable instance {} for container {}",
                    id, model.id
                );
                id
            }
            None => {
                info!(
                    "[Ec2 Controller] No suitable instance found for container {}. Provisioning a new one.",
                    model.id
                );
                self.provision_new_instance(&model, db).await?
            }
        };

        // 2. Deploy the container to the chosen instance
        self.deploy_container_to_instance(&instance_id, &model, db)
            .await?;

        // 3. Update the database to link container and instance
        Mutation::update_container_resource_name(db, model.id.clone(), instance_id.clone()).await?;

        info!(
            "[Ec2 Controller] Scheduled container {} on instance {}",
            model.id, instance_id
        );

        Ok(instance_id)
    }

    async fn is_ssh_accessible(&self, model: &containers::Model) -> Result<bool, std::io::Error> {
        let hostname = self.get_tailscale_device_name(model).await;
        let cmd = vec!["echo".to_string(), "hello".to_string()];
        match run_ssh_command_ts(&hostname, cmd, false, false, Some("ec2-user")) {
            Ok(_) => Ok(true),
            Err(e) => {
                debug!("[EC2 Controller] SSH check failed for {}: {}", hostname, e);
                Ok(false)
            }
        }
    }

    async fn check_done_file(
        &self,
        model: &containers::Model,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let hostname = self.get_tailscale_device_name(model).await;
        let container_id = &model.id;
        let cmd = format!("docker exec {} test -f /done.txt", container_id);
        let cmd_parts = cmd.split_whitespace().map(|s| s.to_string()).collect();

        match run_ssh_command_ts(&hostname, cmd_parts, false, false, Some("ec2-user")) {
            Ok(output) => Ok(output.trim() == "1"),
            Err(_) => Ok(false), // If command fails, file doesn't exist or container isn't running
        }
    }

    async fn perform_health_check(
        &self,
        container: &containers::Model,
        health_check: &V1ContainerHealthCheck,
        db: &DatabaseConnection,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let hostname = self.get_tailscale_device_name(container).await;
        let port = health_check.port.unwrap_or(80);
        let path = health_check.path.as_deref().unwrap_or("/");
        let url = format!("http://{}:{}{}", hostname, port, path);

        let is_ready = match reqwest::get(&url).await {
            Ok(resp) if resp.status().is_success() => true,
            _ => false,
        };

        Mutation::update_container_status(
            db,
            container.id.clone(),
            None,
            None,
            None,
            None,
            None,
            None,
            Some(is_ready),
        )
        .await?;
        Ok(())
    }

    pub async fn watch(
        &self,
        db: &DatabaseConnection,
        container: containers::Model,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let instance_id = container
            .resource_name
            .clone()
            .ok_or("Container has no resource_name (instance_id) to watch")?;

        let mut interval = tokio::time::interval(std::time::Duration::from_secs(10));
        let mut consecutive_errors = 0;
        const MAX_ERRORS: u32 = 5;

        loop {
            interval.tick().await;

            let describe_output = self
                .ec2_client
                .describe_instances()
                .instance_ids(&instance_id)
                .send()
                .await;

            let instance = match describe_output {
                Ok(output) => output
                    .reservations()
                    .first()
                    .and_then(|r| r.instances().first())
                    .cloned(),
                Err(e) => {
                    consecutive_errors += 1;
                    error!(
                        "[EC2 Watcher] Error describing instance {}: {}. (Error {}/{})",
                        instance_id, e, consecutive_errors, MAX_ERRORS
                    );
                    if consecutive_errors >= MAX_ERRORS {
                        // Mark container as Failed
                        break;
                    }
                    continue;
                }
            };

            consecutive_errors = 0;

            let (ec2_status, mut final_status, mut is_ready) = if let Some(inst) = instance {
                let state = inst.state().and_then(|s| s.name()).cloned();
                let status = match state {
                    Some(InstanceStateName::Running) => ContainerStatus::Running,
                    Some(InstanceStateName::Pending) => ContainerStatus::Creating,
                    Some(InstanceStateName::Stopping) | Some(InstanceStateName::ShuttingDown) => {
                        ContainerStatus::Stopped
                    }
                    Some(InstanceStateName::Stopped) => ContainerStatus::Stopped,
                    Some(InstanceStateName::Terminated) => ContainerStatus::Completed,
                    _ => ContainerStatus::Invalid,
                };
                (Some(state), status, false)
            } else {
                // Instance not found, assume terminated
                (None, ContainerStatus::Completed, false)
            };

            if final_status == ContainerStatus::Running {
                if self.is_ssh_accessible(&container).await? {
                    if let Some(hc) = container.parse_health_check()? {
                        self.perform_health_check(&container, &hc, db).await?;
                    } else {
                        is_ready = true; // No health check, SSH is up, so it's ready
                    }

                    if container.restart.to_lowercase() == "never"
                        && self.check_done_file(&container).await?
                    {
                        info!(
                            "[EC2 Watcher] /done.txt found for container {}, deleting.",
                            container.id
                        );
                        self.delete(&container.id, db).await?;
                        break; // Exit watch loop
                    }
                }
            }

            let current_status_str = container
                .parse_status()?
                .and_then(|s| s.status)
                .unwrap_or_default();
            if final_status.to_string() != current_status_str {
                Mutation::update_container_status(
                    db,
                    container.id.clone(),
                    Some(final_status.to_string()),
                    None,
                    None,
                    None,
                    None,
                    None,
                    Some(is_ready),
                )
                .await?;
            }

            if final_status.is_inactive() {
                info!(
                    "[EC2 Watcher] Instance {} reached terminal state: {:?}. Stopping watch.",
                    instance_id, ec2_status
                );
                break;
            }
        }

        Ok(())
    }
}

impl ContainerPlatform for Ec2Platform {
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
        info!("[Ec2 Controller] Using name: {:?}", name);

        let mut ec2_instance_type = "t2.micro".to_string(); // Default value

        // Parse accelerators if provided
        if let Some(accelerators) = &config.accelerators {
            if !accelerators.is_empty() {
                let accelerator = accelerators.get(0).unwrap();
                let parts: Vec<&str> = accelerator.split(':').collect();
                if parts.len() == 2 {
                    if let Some(instance_type) = self.accelerator_map().get(parts[1]) {
                        ec2_instance_type = instance_type.clone();
                    }
                }
            }
        }

        let id = ShortUuid::generate().to_string();
        info!("[Ec2 Controller] ID: {}", id);

        debug!("[Ec2 Controller] About to store agent key secret");
        if user_profile.token.is_none() {
            error!(
                "[Ec2 Controller] user_profile.token is None, cannot get agent key for container"
            );
            return Err(Box::<dyn std::error::Error + Send + Sync>::from(
                "Cannot create container: user profile is missing authentication token".to_string(),
            ));
        }

        debug!(
            "[Ec2 Controller] Storing agent key secret: id={}, owner_id={}",
            id, owner_id
        );
        match self
            .store_agent_key_secret(db, user_profile, &id, owner_id, api_key)
            .await
        {
            Ok(_) => debug!("[Ec2 Controller] Successfully stored agent key secret"),
            Err(e) => {
                error!("[Ec2 Controller] Failed to store agent key secret: {}", e);
                return Err(Box::<dyn std::error::Error + Send + Sync>::from(format!(
                    "Failed to store agent key secret: {}",
                    e
                )));
            }
        }

        let owner_ref: Option<String> = config
            .metadata
            .as_ref()
            .and_then(|meta| meta.owner_ref.clone());

        debug!(
            "[Ec2 Controller] Creating container record in database with instance type: {}",
            ec2_instance_type
        );

        // Create the container record in the database
        let container = crate::entities::containers::ActiveModel {
            id: Set(id.clone()),
            namespace: Set(namespace.to_string()),
            name: Set(name.clone()),
            full_name: Set(format!("{}/{}", namespace, name)),
            owner: Set(owner_id.to_string()),
            owner_ref: Set(owner_ref.clone()),
            image: Set(config.image.clone()),
            env: Set(config.env.clone().map(|vars| serde_json::json!(vars))),
            volumes: Set(config.volumes.clone().map(|vols| serde_json::json!(vols))),
            local_volumes: Set(None),
            accelerators: Set(config.accelerators.clone()),
            cpu_request: Set(None),
            memory_request: Set(None),
            status: Set(Some(serde_json::json!(V1ContainerStatus {
                status: Some(ContainerStatus::Defined.to_string()),
                message: None,
                accelerator: Some(ec2_instance_type.clone()),
                public_ports: None,
                cost_per_hr: None,
                tailnet_url: None,
                ready: None,
            }))),
            platform: Set(Some("ec2".to_string())),
            platforms: Set(None),
            meters: Set(config
                .meters
                .clone()
                .map(|meters| serde_json::json!(meters))),
            resource_name: Set(None),
            resource_namespace: Set(None),
            resource_cost_per_hr: Set(None),
            command: Set(config.command.clone()),
            args: Set(config.args.clone()),
            labels: Set(config
                .metadata
                .as_ref()
                .and_then(|meta| meta.labels.clone().map(|labels| serde_json::json!(labels)))),
            restart: Set(config.restart.clone()),
            queue: Set(config.queue.clone()),
            timeout: Set(config.timeout.clone()),
            resources: Set(config
                .resources
                .clone()
                .map(|resources| serde_json::json!(resources))),
            health_check: Set(config
                .health_check
                .clone()
                .map(|health_check| serde_json::json!(health_check))),
            desired_status: Set(Some(ContainerStatus::Running.to_string())),
            ssh_keys: Set(config.ssh_keys.clone().map(|keys| serde_json::json!(keys))),
            public_addr: Set(None),
            tailnet_ip: Set(None),
            authz: Set(config.authz.clone().map(|authz| serde_json::json!(authz))),
            ports: Set(config.ports.clone().map(|ports| serde_json::json!(ports))),
            proxy_port: Set(config.proxy_port.clone()),
            container_user: Set(None),
            created_by: Set(Some(owner_id.to_string())),
            updated_at: Set(chrono::Utc::now().into()),
            created_at: Set(chrono::Utc::now().into()),
            controller_data: Set(None),
        };

        if let Err(e) = container.insert(db).await {
            error!(
                "[Ec2 Controller] Failed to create container in database: {:?}",
                e
            );
            return Err(format!("Failed to create container in database: {:?}", e).into());
        } else {
            info!("[Ec2 Controller] Created container {} in database ", id);
        }

        Ok(V1Container {
            kind: "Container".to_string(),
            metadata: crate::models::V1ResourceMeta {
                name: name.clone(),
                namespace: namespace.to_string(),
                id: id.clone(),
                owner: owner_id.to_string(),
                owner_ref: owner_ref.clone(),
                created_at: chrono::Utc::now().timestamp(),
                updated_at: chrono::Utc::now().timestamp(),
                created_by: owner_id.to_string(),
                labels: config
                    .metadata
                    .as_ref()
                    .and_then(|meta| meta.labels.clone()),
            },
            image: config.image.clone(),
            platform: "ec2".to_string(),
            env: config.env.clone(),
            command: config.command.clone(),
            args: config.args.clone(),
            volumes: config.volumes.clone(),
            accelerators: config.accelerators.clone(),
            meters: config.meters.clone(),
            queue: config.queue.clone(),
            timeout: config.timeout.clone(),
            ssh_keys: config.ssh_keys.clone(),
            status: Some(V1ContainerStatus {
                status: Some(ContainerStatus::Defined.to_string()),
                message: None,
                accelerator: Some(ec2_instance_type.clone()),
                public_ports: None,
                cost_per_hr: None,
                tailnet_url: None,
                ready: None,
            }),
            restart: config.restart.clone(),
            resources: config.resources.clone(),
            health_check: config.health_check.clone(),
            ports: config.ports.clone(),
            proxy_port: config.proxy_port.clone(),
            authz: config.authz.clone(),
        })
    }
    async fn reconcile(
        &self,
        container: &containers::Model,
        db: &DatabaseConnection,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        debug!(
            "[DEBUG:ec2.rs:reconcile] Entering reconcile for container {}",
            container.id
        );

        if let Ok(Some(parsed_status)) = container.parse_status() {
            let status_str = parsed_status
                .status
                .unwrap_or(ContainerStatus::Invalid.to_string());
            debug!(
                "[DEBUG:ec2.rs:reconcile] Container {} has status {}",
                container.id, status_str
            );

            let status = ContainerStatus::from_str(&status_str).unwrap_or(ContainerStatus::Invalid);

            if status.needs_start() {
                info!(
                    "[Ec2 Controller] Container {} needs to be started",
                    container.id
                );
                if let Some(ds) = &container.desired_status {
                    if ds == &ContainerStatus::Running.to_string() {
                        info!("[Ec2 Controller] Container {} has a desired status of 'running', creating...", container.id);
                        self.create(db, container.clone()).await?;
                    }
                } else {
                    info!(
                        "[Ec2 Controller] Container {} does not have a desired status of 'running'",
                        container.id
                    );
                }
            }

            if status.needs_watch() {
                info!(
                    "[Ec2 Controller] Container {} needs to be watched",
                    container.id
                );
                self.watch(db, container.clone()).await?;
            }
        }
        debug!(
            "[DEBUG:ec2.rs:reconcile] Completed reconcile for container {}",
            container.id
        );
        Ok(())
    }

    async fn exec(
        &self,
        _container_id: &str,
        _command: &str,
        _db: &DatabaseConnection,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        todo!()
    }

    async fn logs(
        &self,
        _container_id: &str,
        _db: &DatabaseConnection,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        todo!()
    }

    async fn delete(
        &self,
        _id: &str,
        _db: &DatabaseConnection,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        todo!()
    }

    fn accelerator_map(&self) -> HashMap<String, String> {
        // Example mapping, will need to be populated with real EC2 instance types
        let mut map = HashMap::new();
        map.insert("NVIDIA_TESLA_T4".to_string(), "g4dn.xlarge".to_string());
        map.insert("NVIDIA_A10G".to_string(), "g5.xlarge".to_string());
        map
    }
}

// This is a simplification. In a real system, you'd get this from a config or a service.
fn get_instance_capacity(instance_type: &aws_sdk_ec2::types::InstanceType) -> (f64, f64) {
    match instance_type.as_str() {
        "t2.micro" => (1.0, 1.0),     // 1 vCPU, 1 GiB RAM
        "t2.small" => (1.0, 2.0),     // 1 vCPU, 2 GiB RAM
        "t3.medium" => (2.0, 4.0),    // 2 vCPU, 4 GiB RAM
        "g4dn.xlarge" => (4.0, 16.0), // 4 vCPU, 16 GiB RAM
        "g5.xlarge" => (4.0, 16.0),
        _ => (0.0, 0.0), // Default to 0, will not match
    }
}
