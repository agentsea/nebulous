use crate::entities::containers;
use crate::models::V1ResourceMeta;
use crate::models::V1UserProfile;
use crate::resources::v1::containers::base::{ContainerPlatform, ContainerStatus};
use crate::resources::v1::containers::models::{
    V1Container, V1ContainerRequest, V1ContainerStatus,
};
use k8s_openapi::api::batch::v1::{Job, JobSpec};
use k8s_openapi::api::core::v1::{
    Container as K8sContainer, ContainerPort, EnvVar, PodSpec, PodTemplateSpec,
    ResourceRequirements, Volume, VolumeMount,
};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use kube::{api::PostParams, Api, Client};
use petname;
use sea_orm::{ActiveModelTrait, DatabaseConnection, Set};
use short_uuid::ShortUuid;
use std::collections::{BTreeMap, HashMap};
use tracing::{error, info};

/// A `ContainerPlatform` implementation that schedules container jobs on Kubernetes.
#[derive(Clone)]
pub struct KubePlatform {
    namespace: String,
    kubeconfig_path: Option<String>,
    context: Option<String>,
}

impl KubePlatform {
    pub fn new() -> Self {
        // Read the namespace from environment variables or use default
        let namespace = std::env::var("KUBE_NAMESPACE").unwrap_or_else(|_| "default".to_string());
        let kubeconfig_path = std::env::var("KUBECONFIG").ok();
        let context = std::env::var("KUBE_CONTEXT").ok();

        KubePlatform {
            namespace,
            kubeconfig_path,
            context,
        }
    }

    /// Create a new KubePlatform with a specific namespace
    pub fn with_namespace(namespace: String) -> Self {
        KubePlatform {
            namespace,
            kubeconfig_path: None,
            context: None,
        }
    }

    /// Create a new KubePlatform with custom configuration
    pub fn with_config(
        namespace: String,
        kubeconfig_path: Option<String>,
        context: Option<String>,
    ) -> Self {
        KubePlatform {
            namespace,
            kubeconfig_path,
            context,
        }
    }

    /// Get a configured Kubernetes client
    async fn get_client(&self) -> Result<Client, kube::Error> {
        if let Some(kubeconfig_path) = &self.kubeconfig_path {
            info!(
                "[Kubernetes] Using kubeconfig from path: {}",
                kubeconfig_path
            );
            // Load kubeconfig from the specified path
            let kubeconfig = match kube::config::Kubeconfig::read_from(kubeconfig_path) {
                Ok(config) => config,
                Err(e) => {
                    return Err(kube::Error::Api(kube::error::ErrorResponse {
                        status: "Failure".to_string(),
                        message: format!("Failed to load kubeconfig: {}", e),
                        reason: "InvalidConfiguration".to_string(),
                        code: 500,
                    }))
                }
            };

            // Create config with optional context
            let config = match &self.context {
                Some(context) => {
                    info!("[Kubernetes] Using context: {}", context);
                    let options = kube::config::KubeConfigOptions {
                        context: Some(context.clone()),
                        ..Default::default()
                    };
                    match kube::config::Config::from_custom_kubeconfig(kubeconfig, &options).await {
                        Ok(config) => config,
                        Err(e) => {
                            return Err(kube::Error::Api(kube::error::ErrorResponse {
                                status: "Failure".to_string(),
                                message: format!("Failed to create config with context: {}", e),
                                reason: "InvalidConfiguration".to_string(),
                                code: 500,
                            }))
                        }
                    }
                }
                None => {
                    let options = kube::config::KubeConfigOptions::default();
                    match kube::config::Config::from_custom_kubeconfig(kubeconfig, &options).await {
                        Ok(config) => config,
                        Err(e) => {
                            return Err(kube::Error::Api(kube::error::ErrorResponse {
                                status: "Failure".to_string(),
                                message: format!("Failed to create config: {}", e),
                                reason: "InvalidConfiguration".to_string(),
                                code: 500,
                            }))
                        }
                    }
                }
            };

            Client::try_from(config)
        } else {
            // Use the default configuration
            Client::try_default().await
        }
    }

    /// Watch a job and update its status in the database
    pub async fn watch_job_status(
        &self,
        job_name: &str,
        container_id: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        info!(
            "[Kubernetes] Starting to watch job {} for container_id {}",
            job_name, container_id
        );

        // Initial status check
        let mut last_status = String::new();
        let mut consecutive_errors = 0;
        const MAX_ERRORS: usize = 5;

        // Get a database connection from the pool
        let db = sea_orm::Database::connect(
            std::env::var("DATABASE_URL").expect("DATABASE_URL must be set"),
        )
        .await?;

        // Get the Kubernetes client
        let client = Client::try_default().await?;
        let jobs: Api<Job> = Api::namespaced(client.clone(), &self.namespace);

        // Poll the job status every 30 seconds
        loop {
            match jobs.get(job_name).await {
                Ok(job) => {
                    consecutive_errors = 0;

                    // Extract status information
                    let current_status = if let Some(status) = &job.status {
                        if let Some(_completion_time) = &status.completion_time {
                            if status.succeeded.unwrap_or(0) > 0 {
                                "completed".to_string()
                            } else {
                                "failed".to_string()
                            }
                        } else if status.active.unwrap_or(0) > 0 {
                            "running".to_string()
                        } else if status.failed.unwrap_or(0) > 0 {
                            "failed".to_string()
                        } else {
                            "pending".to_string()
                        }
                    } else {
                        "pending".to_string()
                    };

                    // If status changed, update the database
                    if current_status != last_status {
                        info!(
                            "[Kubernetes] Job {} status changed: {} -> {}",
                            job_name, last_status, current_status
                        );
                        last_status = current_status.clone();

                        // Update the database with the new status
                        match crate::mutation::Mutation::update_container_status(
                            &db,
                            container_id.to_string(),
                            Some(current_status.clone()),
                            None,
                            None,
                            None,
                            None,
                            None,
                            None,
                        )
                        .await
                        {
                            Ok(_) => {
                                info!(
                                    "[Kubernetes] Updated container {} status to {}",
                                    container_id, current_status
                                )
                            }
                            Err(e) => {
                                error!(
                                    "[Kubernetes] Failed to update job status in database: {}",
                                    e
                                )
                            }
                        }

                        // If the job is in a terminal state, exit the loop
                        if current_status == "completed" || current_status == "failed" {
                            info!(
                                "[Kubernetes] Job {} reached terminal state: {}",
                                job_name, current_status
                            );
                            break;
                        }
                    }
                }
                Err(e) => {
                    error!("[Kubernetes] Error fetching job status: {}", e);
                    consecutive_errors += 1;

                    // If we've had too many consecutive errors, mark the job as failed
                    if consecutive_errors >= MAX_ERRORS {
                        error!("[Kubernetes] Too many consecutive errors, marking job as failed");

                        if let Err(e) = crate::mutation::Mutation::update_container_status(
                            &db,
                            container_id.to_string(),
                            Some("failed".to_string()),
                            Some("Too many consecutive errors".to_string()),
                            None,
                            None,
                            None,
                            None,
                            Some(false),
                        )
                        .await
                        {
                            error!(
                                "[Kubernetes] Failed to update job status in database: {}",
                                e
                            );
                        }

                        break;
                    }
                }
            }

            // Wait before checking again
            tokio::time::sleep(tokio::time::Duration::from_secs(30)).await;
        }

        info!(
            "[Kubernetes] Finished watching job {} for container_id {}",
            job_name, container_id
        );
        Ok(())
    }

    /// Get common environment variables for all containers
    fn get_common_env(&self) -> HashMap<String, String> {
        let mut env = HashMap::new();

        // Add common environment variables here
        env.insert("PLATFORM".to_string(), "kubernetes".to_string());

        env
    }
}

impl ContainerPlatform for KubePlatform {
    /// Run a container on Kubernetes by creating a Job
    async fn declare(
        &self,
        config: &V1ContainerRequest,
        db: &DatabaseConnection,
        _user_profile: &V1UserProfile,
        owner_id: &str,
        namespace: &str,
        api_key: Option<String>,
    ) -> Result<V1Container, Box<dyn std::error::Error + Send + Sync>> {
        let name = config
            .metadata
            .as_ref()
            .and_then(|meta| Some(meta.name.clone()))
            .unwrap_or_else(|| {
                // Generate a random human-friendly name using petname
                petname::petname(3, "-")
            });
        let owner_ref: Option<String> = config
            .metadata
            .as_ref()
            .and_then(|meta| meta.owner_ref.clone());
        info!("[Kubernetes] Using name: {:?}", name);

        // Create a runtime to handle the async call
        let rt = tokio::runtime::Runtime::new().expect("Failed to create Tokio runtime");

        // Determine GPU requirements
        let mut gpu_count = 0;
        let mut gpu_type = "nvidia-tesla-t4"; // Default GPU type

        // Parse accelerators if provided
        if let Some(accelerators) = &config.accelerators {
            if !accelerators.is_empty() {
                // Parse the first accelerator in the list (format: "count:type")
                let parts: Vec<&str> = accelerators[0].split(':').collect();
                if parts.len() == 2 {
                    if let Ok(count) = parts[0].parse::<i32>() {
                        gpu_count = count;
                    }

                    // Convert from our accelerator name to Kubernetes GPU type
                    if let Some(k8s_gpu_name) = self.accelerator_map().get(parts[1]) {
                        gpu_type = k8s_gpu_name;
                        info!(
                            "[Kubernetes] Using accelerator: {} (count: {})",
                            gpu_type, gpu_count
                        );
                    } else {
                        error!(
                            "[Kubernetes] Unknown accelerator type: {}, using default",
                            parts[1]
                        );
                    }
                }
            }
        }

        // Prepare environment variables
        let mut env = Vec::new();

        // Add common environment variables
        for (key, value) in self.get_common_env() {
            env.push(EnvVar {
                name: key,
                value: Some(value),
                ..Default::default()
            });
        }

        // Add ORIGN_SYNC_CONFIG environment variable with serialized volumes configuration
        if let Ok(serialized_volumes) = serde_yaml::to_string(&config.volumes) {
            env.push(EnvVar {
                name: "ORIGN_SYNC_CONFIG".to_string(),
                value: Some(serialized_volumes),
                ..Default::default()
            });
            info!("[Kubernetes] Added ORIGN_SYNC_CONFIG environment variable");
        } else {
            error!("[Kubernetes] Failed to serialize volumes configuration");
        }

        // Add user-provided environment variables
        if let Some(user_env) = &config.env {
            for env_var in user_env {
                env.push(EnvVar {
                    name: env_var.key.clone(),
                    value: env_var.value.clone(), // TODO: Fix this
                    ..Default::default()
                });
            }
        }

        // Prepare volume mounts
        let volume_mounts = vec![
            VolumeMount {
                name: "huggingface-cache".to_string(),
                mount_path: "/huggingface".to_string(),
                ..Default::default()
            },
            VolumeMount {
                name: "nebu-pvc".to_string(),
                mount_path: "/nebu".to_string(),
                ..Default::default()
            },
        ];

        // Prepare volumes
        let volumes = vec![
            Volume {
                name: "huggingface-cache".to_string(),
                persistent_volume_claim: Some(
                    k8s_openapi::api::core::v1::PersistentVolumeClaimVolumeSource {
                        claim_name: "huggingface-cache-pvc".to_string(),
                        ..Default::default()
                    },
                ),
                ..Default::default()
            },
            Volume {
                name: "nebu-pvc".to_string(),
                persistent_volume_claim: Some(
                    k8s_openapi::api::core::v1::PersistentVolumeClaimVolumeSource {
                        claim_name: "nebu-pvc".to_string(),
                        ..Default::default()
                    },
                ),
                ..Default::default()
            },
        ];

        // Prepare node selector for GPU scheduling
        let mut node_selector = BTreeMap::new();
        node_selector.insert("role".to_string(), "gpu".to_string());

        if gpu_count > 0 {
            node_selector.insert("gpu-type".to_string(), "nvidia".to_string());
        }

        // Prepare resource requirements
        let mut resource_requirements = ResourceRequirements::default();

        if gpu_count > 0 {
            let mut limits = BTreeMap::new();
            limits.insert(
                format!("nvidia.com/gpu"),
                k8s_openapi::apimachinery::pkg::api::resource::Quantity(gpu_count.to_string()),
            );
            resource_requirements.limits = Some(limits);
        }

        // Create the container
        let container = K8sContainer {
            name: name.clone().unwrap(),
            image: Some(config.image.clone()),
            command: config
                .command
                .as_ref()
                .map(|cmd| cmd.split(" ").map(String::from).collect()),
            ports: Some(vec![ContainerPort {
                container_port: 8000,
                ..Default::default()
            }]),
            env: Some(env),
            resources: Some(resource_requirements),
            volume_mounts: Some(volume_mounts),
            ..Default::default()
        };

        // Create the pod spec
        let pod_spec = PodSpec {
            containers: vec![container],
            restart_policy: Some("Never".to_string()),
            volumes: Some(volumes),
            node_selector: Some(node_selector),
            ..Default::default()
        };

        // Create the pod template
        let template = PodTemplateSpec {
            metadata: Some(ObjectMeta {
                labels: Some({
                    let mut lbls = BTreeMap::new();
                    lbls.insert("app".to_string(), name.clone().unwrap());
                    lbls
                }),
                ..Default::default()
            }),
            spec: Some(pod_spec),
        };

        // Create the job spec
        let job_spec = JobSpec {
            template,
            backoff_limit: Some(0),
            ..Default::default()
        };

        // Create the job
        let job = Job {
            metadata: ObjectMeta {
                name: Some(name.clone().unwrap()),
                ..Default::default()
            },
            spec: Some(job_spec),
            ..Default::default()
        };

        let id = ShortUuid::generate().to_string();

        // Submit the job to Kubernetes
        rt.block_on(async {
            match self.get_client().await {
                Ok(client) => {
                    let jobs: Api<Job> = Api::namespaced(client, &self.namespace);
                    match jobs.create(&PostParams::default(), &job).await {
                        Ok(_) => {
                            info!("[Kubernetes] Successfully created Job '{:?}'", name);

                            let namespace = config
                                .metadata
                                .as_ref()
                                .and_then(|meta| meta.namespace.clone())
                                .unwrap_or_else(|| _user_profile.email.clone());

                            // Create the container record in the database
                            let container = crate::entities::containers::ActiveModel {
                                id: Set(id.clone()),
                                namespace: Set(namespace.clone()),
                                name: Set(name.clone().unwrap()),
                                full_name: Set(format!("{}/{}", namespace, name.clone().unwrap())),
                                owner: Set(owner_id.to_string()),
                                owner_ref: Set(owner_ref.clone()),
                                image: Set(config.image.clone()),
                                env: Set(config.env.clone().map(|vars| serde_json::json!(vars))),
                                volumes: Set(config
                                    .volumes
                                    .clone()
                                    .map(|vols| serde_json::json!(vols))),
                                local_volumes: Set(None),
                                accelerators: Set(config.accelerators.clone()),
                                cpu_request: Set(None),
                                memory_request: Set(None),
                                status: Set(Some(serde_json::json!(V1ContainerStatus {
                                    status: Some(ContainerStatus::Pending.to_string()),
                                    message: None,
                                    accelerator: None,
                                    public_ports: None,
                                    cost_per_hr: None,
                                    tailnet_url: None,
                                    ready: None,
                                }))),
                                meters: Set(config
                                    .meters
                                    .clone()
                                    .map(|meters| serde_json::json!(meters))),
                                platform: Set(Some("kubernetes".to_string())),
                                platforms: Set(None),
                                resource_name: Set(Some(name.clone().unwrap())),
                                resource_namespace: Set(Some(self.namespace.clone())),
                                resource_cost_per_hr: Set(None),
                                restart: Set(config.restart.clone()),
                                command: Set(config.command.clone()),
                                args: Set(config.args.clone()),
                                queue: Set(config.queue.clone()),
                                timeout: Set(config.timeout.clone()),
                                desired_status: Set(Some("pending".to_string())),
                                controller_data: Set(None),
                                container_user: Set(None),
                                public_addr: Set(None),
                                tailnet_ip: Set(None),
                                authz: Set(config
                                    .authz
                                    .clone()
                                    .map(|authz| serde_json::json!(authz))),
                                ports: Set(config
                                    .ports
                                    .clone()
                                    .map(|ports| serde_json::json!(ports))),
                                proxy_port: Set(config.proxy_port.clone()),
                                resources: Set(config
                                    .resources
                                    .clone()
                                    .map(|resources| serde_json::json!(resources))),
                                health_check: Set(config
                                    .health_check
                                    .clone()
                                    .map(|health_check| serde_json::json!(health_check))),
                                labels: Set(config
                                    .metadata
                                    .as_ref()
                                    .and_then(|meta| meta.labels.clone())
                                    .map(|labels| serde_json::json!(labels))),
                                ssh_keys: Set(config
                                    .ssh_keys
                                    .clone()
                                    .map(|keys| serde_json::json!(keys))),
                                created_by: Set(Some("kubernetes".to_string())),
                                updated_at: Set(chrono::Utc::now().into()),
                                created_at: Set(chrono::Utc::now().into()),
                            };

                            if let Err(e) = container.insert(db).await {
                                error!(
                                    "[Kubernetes] Failed to create container in database: {:?}",
                                    e
                                );
                            } else {
                                info!("[Kubernetes] Created container {:?} in database", name);
                            }

                            // Start watching the job status
                            let name_clone = name.clone();
                            let self_clone = self.clone();

                            tokio::spawn(async move {
                                if let Some(name_str) = &name_clone {
                                    if let Err(e) =
                                        self_clone.watch_job_status(name_str, name_str).await
                                    {
                                        error!("[Kubernetes] Error watching job status: {:?}", e);
                                    }
                                }
                            });
                        }
                        Err(e) => {
                            error!("[Kubernetes] Error creating Job '{:?}': {:?}", name, e);
                        }
                    }
                }
                Err(e) => {
                    error!("[Kubernetes] Failed to create K8s client: {:?}", e);
                }
            }
        });

        info!("[Kubernetes] Job {:?} created on Kubernetes", name);
        Ok(V1Container {
            kind: "Container".to_string(),
            metadata: V1ResourceMeta {
                name: name.clone().unwrap(),
                namespace: namespace.to_string(),
                id: id.clone(),
                owner: owner_id.to_string(),
                owner_ref: owner_ref.clone(),
                created_at: chrono::Utc::now().timestamp(),
                updated_at: chrono::Utc::now().timestamp(),
                created_by: "kubernetes".to_string(),
                labels: config
                    .metadata
                    .as_ref()
                    .and_then(|meta| meta.labels.clone()),
            },
            image: config.image.clone(),
            env: config.env.clone(),
            command: config.command.clone(),
            args: config.args.clone(),
            platform: config.platform.clone().unwrap_or_default(),
            volumes: config.volumes.clone(),
            accelerators: config.accelerators.clone(),
            meters: config.meters.clone(),
            queue: config.queue.clone(),
            timeout: config.timeout.clone(),
            ssh_keys: config.ssh_keys.clone(),
            status: Some(V1ContainerStatus {
                status: Some(ContainerStatus::Pending.to_string()),
                message: None,
                accelerator: None,
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
        Ok(())
    }

    async fn exec(
        &self,
        container_id: &str,
        command: &str,
        db: &DatabaseConnection,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        Ok("".to_string())
    }

    async fn logs(
        &self,
        container_id: &str,
        db: &DatabaseConnection,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        Ok("".to_string())
    }

    async fn delete(
        &self,
        id: &str,
        db: &DatabaseConnection,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }

    fn accelerator_map(&self) -> HashMap<String, String> {
        return HashMap::new();
    }
}
