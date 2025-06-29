use crate::agent::agent::create_agent_key;
use crate::config::{ClientConfig, SERVER_CONFIG};
use crate::entities::containers;
use crate::entities::processors;
use crate::models::V1CreateAgentKeyRequest;
use crate::models::V1UserProfile;
use crate::mutation::Mutation;
use crate::query::Query;
use crate::resources::v1::containers::base::get_ip_for_tailscale_device_hostname;
use crate::resources::v1::containers::base::ContainerStatus;
use crate::resources::v1::containers::factory::platform_factory;
use crate::resources::v1::containers::models::V1ContainerRequest;
use crate::resources::v1::containers::models::V1EnvVar;
use crate::resources::v1::processors::base::{ProcessorPlatform, ProcessorStatus};
use crate::resources::v1::processors::models::{
    V1Processor, V1ProcessorRequest, V1ProcessorStatus,
};
use crate::state::MessageQueue;
use crate::streams::redis::get_consumer_group_progress;
use crate::AppState;
use chrono::{DateTime, Duration, Utc};
use reqwest;
use sea_orm::{ActiveModelTrait, ActiveValue::Set, DatabaseConnection, EntityTrait};
use short_uuid::ShortUuid;
use std::str::FromStr;
use std::sync::Arc;
use tracing::{debug, error, info, warn};

/// Standard implementation of the ProcessorPlatform trait
pub struct StandardProcessor {
    state: Arc<AppState>,
}

impl StandardProcessor {
    /// Create a new StandardProcessor instance
    pub fn new(state: Arc<AppState>) -> Self {
        Self { state }
    }

    async fn customize_container(
        &self,
        processor: &processors::Model,
        container: Option<V1ContainerRequest>,
        redis_client: &redis::Client,
    ) -> Result<V1ContainerRequest, Box<dyn std::error::Error + Send + Sync>> {
        debug!(
            "[Processor Controller] Customizing container {:?}",
            container
        );
        // Parse container or use default
        let mut parsed_container = match container {
            Some(c) => c,
            None => match processor.parse_container() {
                Ok(Some(c)) => c,
                Ok(None) => {
                    info!("[Processor Controller] Using default container spec");
                    Default::default()
                }
                Err(e) => {
                    return Err(format!(
                        "Failed to parse container JSON for processor {}: {}",
                        processor.id, e
                    )
                    .into());
                }
            },
        };

        let mut metadata = parsed_container.metadata.unwrap_or_default();
        let mut env = parsed_container.env.clone().unwrap_or_default();

        // Use processor ID for username (sanitize for Redis)
        let username = format!("proc_{}", processor.id.replace("-", "_"));

        // TODO: lol
        let password = format!("pass_{}", processor.id.replace("-", ""));

        // Stream key pattern this processor should access
        let stream_pattern = format!("processor:{}:{}*", processor.namespace, processor.name);

        // Connect to Redis
        let mut conn = redis_client.get_connection().map_err(
            |e| -> Box<dyn std::error::Error + Send + Sync> {
                format!("Failed to connect to Redis: {}", e).into()
            },
        )?;

        // Check if user already exists
        let user_exists: bool = redis::cmd("ACL")
            .arg("GETUSER")
            .arg(&username)
            .query(&mut conn)
            .unwrap_or(false);

        // Only create the user if it doesn't exist
        if !user_exists {
            // Define the KV pattern (e.g., using namespace and processor name)
            let kv_pattern = format!("cache:{}:*", processor.namespace);

            // Create ACL user with permissions for streams and KV prefix
            let acl_result: redis::RedisResult<String> = redis::cmd("ACL")
                .arg("SETUSER")
                .arg(&username)
                .arg("on")
                .arg(format!(">{}", &password))
                // Stream access
                .arg(format!("~{}", &stream_pattern)) // Key pattern restriction for streams
                .arg("+@stream") // Grant all stream commands (simpler than listing individuals)
                // KV access
                .arg(format!("~{}", &kv_pattern)) // Key pattern restriction for KV
                .arg("+@string") // Grant string commands (GET, SET, etc.)
                .arg("+del") // Grant DEL command specifically
                // Basic connection check
                .arg("+ping")
                .query(&mut conn);

            match acl_result {
                Ok(_) => info!(
                    "[Processor] Created Redis ACL user for processor {}",
                    processor.id
                ),
                Err(e) => return Err(format!("Failed to create Redis ACL user: {}", e).into()),
            }
        }

        // Add Redis credentials to environment variables
        env.push(V1EnvVar {
            key: "REDIS_USERNAME".to_string(),
            value: Some(username.clone()),
            secret_name: None,
        });

        env.push(V1EnvVar {
            key: "REDIS_PASSWORD".to_string(),
            value: Some(password.clone()),
            secret_name: None,
        });

        env.push(V1EnvVar {
            key: "REDISCLI_AUTH".to_string(),
            value: Some(password.clone()),
            secret_name: None,
        });

        // Fetch Redis IP from Tailscale
        let redis_hostname = "redis"; // Or get this from config if it can vary
        let redis_ip = match get_ip_for_tailscale_device_hostname(redis_hostname).await {
            Ok(ip) => ip,
            Err(e) => {
                error!(
                    "[Processor Controller] Failed to get Tailscale IP for '{}': {}. Falling back to configured REDIS_URL/REDIS_PUBLISH_URL.",
                    redis_hostname,
                    e
                );
                // Fallback to existing config if Tailscale lookup fails
                // This maintains previous behavior in case of Tailscale issues.
                match SERVER_CONFIG.redis.publish_url.clone() {
                    Some(url) => {
                        debug!("Using REDIS_PUBLISH_URL: {}", url);
                        url
                    }
                    None => SERVER_CONFIG.redis.get_url()
                }
            }
        };

        // Construct Redis URL using the fetched IP and default port
        let redis_url = format!("redis://{}:{}@{}:6379", username, password, redis_ip);
        info!("[Processor Controller] Using Redis URL: {}", redis_url);

        // Add all Redis env vars
        env.push(V1EnvVar {
            key: "REDIS_URL".to_string(),
            value: Some(redis_url),
            secret_name: None,
        });
        env.push(V1EnvVar {
            key: "REDIS_CONSUMER_GROUP".to_string(),
            value: Some(processor.id.clone()),
            secret_name: None,
        });
        env.push(V1EnvVar {
            key: "REDIS_STREAM".to_string(),
            value: Some(processor.stream.clone()),
            secret_name: None,
        });

        // Configure labels and metadata
        let mut labels = metadata.labels.clone().unwrap_or_default();
        labels.insert("processor".to_string(), processor.id.clone());
        metadata.labels = Some(labels);
        metadata.owner_ref = Some(format!(
            "{}.{}.Processor",
            processor.name, processor.namespace
        ));
        metadata.namespace = Some(processor.namespace.clone());

        // Update the container
        parsed_container.metadata = Some(metadata);
        parsed_container.env = Some(env);

        Ok(parsed_container)
    }

    /// Start a processor, creating its minimum number of containers on Runpod (example).
    async fn start_processor(
        &self,
        db: &DatabaseConnection,
        processor: processors::Model,
        owner_profile: &V1UserProfile,
        redis_client: &redis::Client,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!("[Processor Controller] Starting processor {}", processor.id);

        // 1) Mark Processor status as Creating (if not already something else)
        //    This might be redundant if reconcile loop already set it, but ensures it's set.
        let current_status_str = processor
            .parse_status()
            .ok()
            .flatten()
            .and_then(|s| s.status)
            .unwrap_or_default();
        if ProcessorStatus::from_str(&current_status_str).unwrap_or(ProcessorStatus::Invalid)
            == ProcessorStatus::Defined
        {
            info!(
                "[Processor Controller] Setting processor {} status to Creating",
                processor.id
            );
            Mutation::update_processor_status(
                db,
                processor.id.clone(),
                Some(ProcessorStatus::Creating.to_string()),
                None,
            )
            .await?;
        } else {
            info!(
                "[Processor Controller] Processor {} status already '{}', not setting to Creating.",
                processor.id, current_status_str
            );
        }

        // 2) Ensure desired_replicas is set based on min_replicas
        let target_replicas = processor.min_replicas.unwrap_or(1).max(1);
        if processor.desired_replicas != Some(target_replicas) {
            info!("[Processor Controller] Setting processor {} desired_replicas to {} based on min_replicas", processor.id, target_replicas);
            let mut active_model = processors::ActiveModel::from(processor.clone());
            active_model.desired_replicas = sea_orm::ActiveValue::Set(Some(target_replicas));
            active_model.update(db).await?;
            // Note: We use the original processor model below, but the update is now in DB for the watch loop
        } else {
            info!("[Processor Controller] Processor {} desired_replicas already matches min_replicas ({})", processor.id, target_replicas);
        }

        // 3) Update Processor desired_status to Running (if not already)
        // The actual status (like Running, Failed etc.) will be updated by the watch loop based on pod states.
        if processor.desired_status != Some(ProcessorStatus::Running.to_string()) {
            info!(
                "[Processor Controller] Setting processor {} desired_status to Running",
                processor.id
            );
            Mutation::update_processor_desired_status(
                db,
                processor.id,
                Some(ProcessorStatus::Running.to_string()),
            )
            .await?;
        } else {
            info!(
                "[Processor Controller] Processor {} desired_status already Running",
                processor.id
            );
        }

        Ok(())
    }

    // Helper function to parse duration string into chrono::Duration
    fn parse_duration(
        &self,
        duration_str: &str,
    ) -> Result<Duration, Box<dyn std::error::Error + Send + Sync>> {
        // Parse duration strings in format like "1m", "30s", "5h"
        let len = duration_str.len();
        if len < 2 {
            return Err(format!("Invalid duration format: {}", duration_str).into());
        }

        let (value_str, unit) = duration_str.split_at(len - 1);
        let value = value_str
            .parse::<i64>()
            .map_err(|e| format!("Invalid duration value: {}", e))?;

        match unit {
            "s" => Ok(Duration::seconds(value)),
            "m" => Ok(Duration::minutes(value)),
            "h" => Ok(Duration::hours(value)),
            "d" => Ok(Duration::days(value)),
            _ => Err(format!("Unsupported duration unit: {}", unit).into()),
        }
    }

    // Helper to check if duration threshold has been met
    #[allow(dependency_on_unit_never_type_fallback)] // TODO: this is due to Redis crate
    async fn duration_threshold_met(
        &self,
        redis_client: &redis::Client,
        processor_id: &str,
        action_type: &str,
        required_duration: &Duration,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let redis_key = format!("processor:{}:scale:{}", processor_id, action_type);
        let mut conn = redis_client.get_connection()?;

        // Check if we already have timestamp in Redis
        let threshold_start: Option<String> = redis::cmd("GET")
            .arg(&redis_key)
            .query::<Option<String>>(&mut conn)
            .unwrap_or(None);

        match threshold_start {
            Some(timestamp_str) => {
                // Parse the stored timestamp
                let threshold_time = DateTime::parse_from_rfc3339(&timestamp_str)
                    .map_err(|e| format!("Invalid timestamp format in Redis: {}", e))?
                    .with_timezone(&Utc);

                // Check if enough time has elapsed
                let elapsed = Utc::now() - threshold_time;
                Ok(elapsed >= *required_duration)
            }
            None => {
                // First time we're seeing this threshold exceeded, store current time
                let now = Utc::now();
                redis::cmd("SET")
                    .arg(&redis_key)
                    .arg(now.to_rfc3339())
                    .query(&mut conn)?;

                // Have not met duration threshold yet
                Ok(false)
            }
        }
    }

    // Helper to clear stored duration threshold
    async fn clear_duration_threshold(
        &self,
        redis_client: &redis::Client,
        processor_id: &str,
        action_type: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        #![allow(dependency_on_unit_never_type_fallback)]
        let redis_key = format!("processor:{}:scale:{}", processor_id, action_type);
        let mut conn = redis_client.get_connection()?;

        redis::cmd("DEL").arg(&redis_key).query(&mut conn)?;

        Ok(())
    }

    /// Watch/monitor a processor and scale containers based on Redis queue 'pressure'.
    async fn watch_processor(
        &self,
        db: &DatabaseConnection,
        processor: processors::Model,
        owner_profile: &V1UserProfile,
        redis_client: &redis::Client,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        use crate::resources::v1::containers::factory::platform_factory;
        use crate::resources::v1::containers::runpod::RunpodPlatform;
        use std::collections::{HashMap, HashSet};
        use tracing::info;

        info!("[Processor Controller] Watching processor {}", processor.id);

        // --- BEGIN Reconciliation between DB and Runpod ---

        // 1. Get expected owner_ref and platform
        let owner_ref_string = format!("{}.{}.Processor", processor.name, processor.namespace);
        // TODO: This assumes Runpod - ideally, determine platform dynamically if needed
        let platform = platform_factory("runpod".to_string()); // Assuming RunpodPlatform

        // Cast to RunpodPlatform to access runpod_client - This is a bit hacky, assumes watch is only for runpod
        // A better approach might involve adding list_pods to ContainerPlatform trait or specific logic
        let runpod_platform = RunpodPlatform::new(); // Creates a new client, consider passing it down if possible

        // 2. Fetch ALL Runpod pods for the user
        let all_runpod_pods_result = runpod_platform.list_runpod_pods().await;
        let all_runpod_pods = match all_runpod_pods_result {
            Ok(pods) => pods.data.map_or(vec![], |d| d.pods),
            Err(e) => {
                error!(
                    "[Processor Controller] Failed to list Runpod pods for reconciliation: {}",
                    e
                );
                // Decide how to handle: return error, or proceed with potentially stale DB data?
                // For now, return error to avoid incorrect scaling.
                return Err(format!("Failed to list Runpod pods: {}", e).into());
            }
        };
        debug!(
            "[Processor Controller] Fetched {} total Runpod pods for user.",
            all_runpod_pods.len()
        );

        // 3. Fetch DB containers for *this* processor
        let db_containers = Query::find_containers_by_owner_ref(db, &owner_ref_string).await?;
        let db_containers_map: HashMap<String, &crate::entities::containers::Model> =
            db_containers.iter().map(|c| (c.id.clone(), c)).collect();
        debug!(
            "[Processor Controller] Found {} DB container records for processor {}",
            db_containers.len(),
            processor.id
        );

        // 4. Correlate Runpod pods with DB containers for this processor
        let mut relevant_runpod_pods_map = HashMap::new();
        let mut runpod_pod_names_for_processor = HashSet::new(); // Track names (IDs) of Runpod pods belonging to this processor

        for pod in all_runpod_pods {
            // The Runpod pod `name` should match the container `id` from our DB
            if db_containers_map.contains_key(&pod.name) {
                debug!(
                    "[Processor Controller] Found matching Runpod pod '{}' for processor {}",
                    pod.name, processor.id
                );
                relevant_runpod_pods_map.insert(pod.name.clone(), pod.clone()); // Use pod name as key (which is container ID)
                runpod_pod_names_for_processor.insert(pod.name.clone());
            }
        }
        debug!(
            "[Processor Controller] Found {} Runpod pods matching DB records for processor {}",
            relevant_runpod_pods_map.len(),
            processor.id
        );

        // 5. Reconcile states and calculate actual replicas
        let mut actual_active_replicas: i32 = 0;
        let mut container_ids_to_mark_failed = Vec::new();
        let mut container_ids_to_delete_pod = Vec::new(); // Use container ID (which is pod name)
        let mut active_runpod_containers: Vec<containers::Model> = Vec::new(); // Collect active containers

        for (container_id, db_container) in &db_containers_map {
            let db_status_opt = db_container.parse_status().unwrap_or(None);
            let db_status = db_status_opt
                .as_ref()
                .and_then(|s| s.status.as_deref())
                .and_then(|s_str| ContainerStatus::from_str(s_str).ok())
                .unwrap_or(ContainerStatus::Invalid); // Default to Invalid if parse fails

            if let Some(runpod_pod) = relevant_runpod_pods_map.get(container_id) {
                // Pod exists in Runpod and DB record exists
                let runpod_desired_status_str = &runpod_pod.desired_status;
                // Map Runpod status string to our ContainerStatus enum
                let runpod_status = match runpod_desired_status_str.as_str() {
                    "RUNNING" => ContainerStatus::Running,
                    "EXITED" => ContainerStatus::Completed,
                    "TERMINATED" => ContainerStatus::Stopped, // Or maybe Deleted? Runpod uses Terminated
                    "DEAD" => ContainerStatus::Failed,
                    "CREATED" => ContainerStatus::Created, // Or Defined?
                    "RESTARTING" => ContainerStatus::Restarting,
                    "PAUSED" => ContainerStatus::Paused,
                    _ => ContainerStatus::Pending, // Default for unknown Runpod statuses
                };

                debug!("[Processor Controller] Reconciling Container ID: {}, DB Status: {:?}, Runpod Status: {:?} ({})",
                       container_id, db_status, runpod_status, runpod_desired_status_str);

                if runpod_status.is_active() && db_status.is_active() {
                    debug!("[Processor Controller] Container {} active in DB and Runpod. Counting as replica.", container_id);
                    actual_active_replicas += 1;
                    active_runpod_containers.push((*db_container).clone()); // Add to list if active
                } else if runpod_status.is_inactive() && db_status.is_active() {
                    // Pod is terminal in Runpod, but DB thinks it's active. Update DB.
                    warn!("[Processor Controller] Container {} is terminal ({}) in Runpod but active ({:?}) in DB. Updating DB.",
                           container_id, runpod_desired_status_str, db_status);
                    container_ids_to_mark_failed.push(container_id.clone()); // Mark as failed for simplicity
                } else if runpod_status.is_active() && db_status.is_inactive() {
                    // Pod is active in Runpod, but DB thinks it's terminal. Delete the pod.
                    warn!("[Processor Controller] Container {} is active ({}) in Runpod but terminal ({:?}) in DB. Deleting Runpod pod.",
                            container_id, runpod_desired_status_str, db_status);
                    container_ids_to_delete_pod.push(container_id.clone());
                }
                // If both are inactive, do nothing - state is consistent.
                // If both are active, we already counted it.
            } else {
                // Pod not found in Runpod, but DB record exists
                if db_status.is_active() {
                    warn!("[Processor Controller] DB Container {} is active ({:?}) but no matching pod found in Runpod. Marking failed in DB.",
                            container_id, db_status);
                    container_ids_to_mark_failed.push(container_id.clone());
                }
                // If DB status is already inactive, do nothing - state is consistent.
            }
        }

        // --- Perform DB Updates and Pod Deletions ---
        for container_id in container_ids_to_mark_failed {
            if let Err(e) = Mutation::update_container_status(
                db,
                container_id.clone(),
                Some(ContainerStatus::Failed.to_string()),
                Some("Associated Runpod pod not found or in terminal state.".to_string()),
                None,
                None,
                None,
                None,
                Some(false), // Mark not ready
            )
            .await
            {
                error!(
                    "[Processor Controller] Failed to mark container {} as Failed in DB: {}",
                    container_id, e
                );
            }
        }

        for container_id_to_delete in container_ids_to_delete_pod {
            warn!(
                "[Processor Controller] Deleting Runpod pod for container ID: {}",
                container_id_to_delete
            );
            // We use the container ID directly as it's the pod name in Runpod
            match platform.delete(&container_id_to_delete, db).await {
                Ok(_) => info!("[Processor Controller] Successfully deleted orphaned/mismatched Runpod pod for container {}", container_id_to_delete),
                Err(e) => error!("[Processor Controller] Failed to delete orphaned/mismatched Runpod pod for container {}: {}", container_id_to_delete, e),
            }
        }
        // --- END Reconciliation ---

        // Use the reconciled count
        let current_replicas = actual_active_replicas;
        info!(
            "[Processor Controller] Reconciled active replicas: {}",
            current_replicas
        );

        // Get desired replicas from processor config (this might be updated later by scaling logic)
        // Initialize desired_replicas based on processor's min_replicas if desired_replicas field is None
        let initial_desired_replicas = processor.desired_replicas.unwrap_or_else(|| {
            processor.min_replicas.unwrap_or(1).max(1) // Ensure at least 1 if min_replicas is None or 0
        });

        // Initial reconcile based on DB settings vs actual count before checking pressure
        // This ensures we reach the processor.desired_replicas count even without scaling triggers.
        if current_replicas != initial_desired_replicas {
            info!("[Processor Controller] Initial reconcile needed: current={}, desired={}. Reconciling...",
                   current_replicas, initial_desired_replicas);
            let container_spec_for_reconcile = match processor.parse_container() {
                Ok(Some(c)) => c,
                Ok(None) => Default::default(),
                Err(e) => {
                    error!("[Processor Controller] Failed to parse container spec during initial reconcile: {}", e);
                    return Err(format!("Failed to parse container spec: {}", e).into());
                }
            };
            self.reconcile_replicas(
                &processor,
                current_replicas, // Use the actual count
                initial_desired_replicas,
                active_runpod_containers.clone(), // Pass the list of active containers
                container_spec_for_reconcile,
                db,
                owner_profile,
                redis_client,
            )
            .await?;
            // Update current_replicas count after this initial reconciliation
            // It might be better to re-fetch the actual count, but for now, assume reconcile worked.
            // current_replicas = initial_desired_replicas; // Or re-run the reconciliation check? Let's assume it's desired for now.
            // Re-fetch might be safer:
            // TODO: Re-run the pod fetching/counting logic here to get the *very latest* count after reconcile_replicas
            // For now, we proceed with the potentially updated DB count, but the *next* watch cycle will catch up.
        }

        // --- Existing Scaling Logic (using reconciled current_replicas) ---

        // Make a connection from the client:
        let mut con = redis_client.get_connection()?;

        // 1) Make sure there's a stream name in the processor.
        let stream_name = processor.stream.clone();

        // 2) The consumer group is the processor's ID.
        let consumer_group = &processor.id;
        debug!("Consumer group: {:?}", consumer_group);

        // 3) Check how many messages are pending for this group in the Redis stream (i.e. 'pressure').
        let pending_count =
            match get_consumer_group_progress(&mut con, &stream_name, consumer_group) {
                Ok(progress) => progress.remaining_entries(),
                Err(err) => {
                    warn!(
                    "[Processor Controller] Error getting pending count for processor {:?}: {:?}",
                    processor.id, err
                );
                    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                    return Ok(()); // Continue watch loop even if pressure check fails once
                }
            };

        debug!("Pending count for {}: {:?}", processor.id, pending_count);

        // 4) Compare `pending_count` to scale.up.pressure and scale.down.pressure.
        let scale = if let Ok(s) = processor.parse_scale() {
            s
        } else {
            None
        };

        // If no scale object, do nothing special for scaling
        let Some(scale) = scale else {
            info!(
                "[Processor Controller] Processor {} has no scale rules; skipping pressure-based scaling",
                processor.id
            );
            // Still need the sleep at the end
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            return Ok(());
        };

        debug!("Scale rules for {}: {:?}", processor.id, scale);

        // Extract scale up/down thresholds.
        let scale_up_threshold = scale
            .up
            .as_ref()
            .and_then(|up| up.above_pressure)
            .unwrap_or(i32::MAX);
        let scale_down_threshold = scale
            .down
            .as_ref()
            .and_then(|down| down.below_pressure)
            .unwrap_or(0);

        // Extract and parse scale durations
        let scale_up_duration = scale
            .up
            .as_ref()
            .and_then(|up| up.duration.clone())
            .map(|d| self.parse_duration(&d))
            .transpose()?
            .unwrap_or(Duration::zero());

        let scale_down_duration = scale
            .down
            .as_ref()
            .and_then(|down| down.duration.clone())
            .map(|d| self.parse_duration(&d))
            .transpose()?
            .unwrap_or(Duration::zero());

        // Get max replicas from processor spec, default to a high number if not set
        let max_replicas = processor.max_replicas.unwrap_or(i32::MAX);
        // Get min replicas from processor spec, default to 1
        let min_replicas = processor.min_replicas.unwrap_or(1).max(1); // Ensure at least 1

        debug!(
            "Scale up threshold: {:?}, duration: {:?}",
            scale_up_threshold, scale_up_duration
        );
        debug!(
            "Scale down threshold: {:?}, duration: {:?}",
            scale_down_threshold, scale_down_duration
        );
        debug!(
            "Min replicas: {}, Max replicas: {}",
            min_replicas, max_replicas
        );

        let mut replica_change_needed = false;
        // Use current_replicas (the reconciled actual count)
        let mut new_replica_target = current_replicas;
        debug!(
            "Current actual replicas: {}, Target before scaling: {}",
            current_replicas, new_replica_target
        );

        // Scale-up check
        if (pending_count as i32) >= scale_up_threshold && current_replicas < max_replicas {
            debug!("Scale-up condition met: pending ({}) >= threshold ({}) AND current ({}) < max ({})",
                    pending_count, scale_up_threshold, current_replicas, max_replicas);
            if scale_up_duration > Duration::zero() {
                let duration_met = self
                    .duration_threshold_met(redis_client, &processor.id, "up", &scale_up_duration)
                    .await?;
                if duration_met {
                    new_replica_target = (current_replicas + 1).min(max_replicas); // Apply max replicas limit
                    info!(
                        "[Processor Controller] Scaling UP processor {} from {} -> {} replicas (duration threshold met)",
                        processor.id, current_replicas, new_replica_target
                    );
                    replica_change_needed = true;
                    self.clear_duration_threshold(redis_client, &processor.id, "up")
                        .await?;
                } else {
                    info!(
                        "[Processor Controller] Processor {} scale-up threshold met, but duration not yet met",
                        processor.id
                    );
                }
            } else {
                // Instant scale up
                new_replica_target = (current_replicas + 1).min(max_replicas); // Apply max replicas limit
                info!(
                    "[Processor Controller] Scaling UP processor {} from {} -> {} replicas (instant)",
                    processor.id, current_replicas, new_replica_target
                );
                replica_change_needed = true;
            }
            // Clear any scale-down tracking when scaling up
            if replica_change_needed {
                // Only clear if we actually decided to scale
                self.clear_duration_threshold(redis_client, &processor.id, "down")
                    .await?;
            }
        }
        // Scale-down check
        else if (pending_count as i32) <= scale_down_threshold && current_replicas > min_replicas
        {
            debug!("Scale-down condition met: pending ({}) <= threshold ({}) AND current ({}) > min ({})",
                    pending_count, scale_down_threshold, current_replicas, min_replicas);
            if scale_down_duration > Duration::zero() {
                let duration_met = self
                    .duration_threshold_met(
                        redis_client,
                        &processor.id,
                        "down",
                        &scale_down_duration,
                    )
                    .await?;
                if duration_met {
                    new_replica_target = (current_replicas - 1).max(min_replicas); // Apply min replicas limit
                    info!(
                        "[Processor Controller] Scaling DOWN processor {} from {} -> {} replicas (duration threshold met)",
                        processor.id, current_replicas, new_replica_target
                    );
                    replica_change_needed = true;
                    self.clear_duration_threshold(redis_client, &processor.id, "down")
                        .await?;
                } else {
                    info!(
                        "[Processor Controller] Processor {} scale-down threshold met, but duration not yet met",
                        processor.id
                    );
                }
            } else {
                // Instant scale down
                new_replica_target = (current_replicas - 1).max(min_replicas); // Apply min replicas limit
                info!(
                    "[Processor Controller] Scaling DOWN processor {} from {} -> {} replicas (instant)",
                    processor.id, current_replicas, new_replica_target
                );
                replica_change_needed = true;
            }
            // Clear any scale-up tracking when scaling down
            if replica_change_needed {
                // Only clear if we actually decided to scale
                self.clear_duration_threshold(redis_client, &processor.id, "up")
                    .await?;
            }
        } else {
            // Not scaling up or down based on pressure, clear both trackers
            debug!("Pressure ({}) is between thresholds [{}, {}] or limits reached [{}, {}]. Clearing scale duration trackers.",
                   pending_count, scale_down_threshold, scale_up_threshold, min_replicas, max_replicas);
            self.clear_duration_threshold(redis_client, &processor.id, "up")
                .await?;
            self.clear_duration_threshold(redis_client, &processor.id, "down")
                .await?;
        }

        // 6) If the replica target changed due to scaling pressure, update DB and reconcile.
        if replica_change_needed && new_replica_target != current_replicas {
            debug!(
                "[Processor Controller] Scaling processor {}: target count changed from {} to {}",
                processor.id, current_replicas, new_replica_target
            );

            // Fetch latest processor model before updating to avoid race conditions
            let latest_processor_model = match processors::Entity::find_by_id(processor.id.clone())
                .one(db)
                .await?
            {
                Some(p) => p,
                None => {
                    error!("[Processor Controller] Processor {} not found before attempting to update desired_replicas.", processor.id);
                    return Err(format!(
                        "Processor {} disappeared during watch cycle.",
                        processor.id
                    )
                    .into());
                }
            };

            let mut active_model = processors::ActiveModel::from(latest_processor_model); // Use latest model
            active_model.desired_replicas = sea_orm::ActiveValue::Set(Some(new_replica_target));
            let updated_model = active_model.update(db).await?; // This updates the processor table

            let parsed_container = match updated_model.parse_container() {
                // Use updated_model
                Ok(Some(c)) => c,
                Ok(None) => Default::default(),
                Err(e) => {
                    error!("[Processor Controller] Failed to parse container spec before reconcile: {}", e);
                    return Err(format!("Failed to parse container spec: {}", e).into());
                }
            };

            info!(
                "[Processor Controller] Updated processor {} desired_replicas to {} in DB; reconciling...",
                updated_model.id, new_replica_target
            );
            self.reconcile_replicas(
                &updated_model,                   // Pass the updated model
                current_replicas,                 // The actual count *before* this reconcile call
                new_replica_target,               // The target count
                active_runpod_containers.clone(), // Pass the list of active containers
                parsed_container,
                db,
                owner_profile,
                redis_client,
            )
            .await?;
        } else {
            info!(
                "[Processor Controller] No scaling change needed for processor {}; actual replicas count = {}",
                processor.id, current_replicas
            );
        }

        // Add a short delay before the next watch cycle
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;

        Ok(())
    }

    async fn reconcile_replicas(
        &self,
        processor: &processors::Model,
        current_replicas: i32,
        new_replica_count: i32,
        active_runpod_containers: Vec<containers::Model>,
        container_request: V1ContainerRequest,
        db: &DatabaseConnection,
        owner_profile: &V1UserProfile,
        redis_client: &redis::Client,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Get the processor's agent key
        let secret_name = format!("processor-agent-key-{}", processor.id);
        let secret_namespace = "root";

        debug!("Fetching secret {}/{}", secret_namespace, secret_name);
        let secret_model =
            Query::find_secret_by_namespace_and_name(db, secret_namespace, &secret_name)
                .await
                .map_err(|e| format!("Database error fetching secret: {}", e))?
                .ok_or_else(|| {
                    format!(
                        "Secret '{}/{}' not found for processor {}",
                        secret_namespace, secret_name, processor.id
                    )
                })?;

        debug!("Decrypting secret value for processor {}", processor.id);
        let agent_key = secret_model
            .decrypt_value()
            .map_err(|e| format!("Failed to decrypt agent key: {}", e))?;

        // Get the customized container with all our environment variables
        let container = self
            .customize_container(processor, Some(container_request), redis_client)
            .await?;

        debug!(
            "[Processor Controller] Processor {} customized container: {:?}",
            processor.id, container
        );

        // Get the appropriate platform for this container
        let platform_str = container.platform.clone().unwrap_or("runpod".to_string());
        let platform = platform_factory(platform_str);

        if new_replica_count > current_replicas {
            // Create containers for the difference between current and new count
            for replica_index in current_replicas..new_replica_count {
                let mut request_for_replica = container.clone();
                if let Some(mut meta) = request_for_replica.metadata.take() {
                    meta.name = Some(format!(
                        "{}-replica-{}-{}",
                        meta.name.unwrap_or_default(),
                        replica_index,
                        ShortUuid::generate()
                            .to_string()
                            .chars()
                            .take(5)
                            .collect::<String>()
                    ));
                    request_for_replica.metadata = Some(meta);
                }

                info!(
                    "[Processor Controller] Creating container #{} for processor {}",
                    replica_index, processor.id
                );
                debug!("Request for replica: {:?}", request_for_replica);

                let declared = platform
                    .declare(
                        &request_for_replica,
                        db,
                        owner_profile,
                        &processor.owner,
                        &processor.namespace,
                        Some(agent_key.clone()),
                    )
                    .await?;

                info!(
                    "[Processor Controller] Created container {} (id = {}) for processor {}",
                    declared.metadata.name, declared.metadata.id, processor.id
                );
            }
        } else if new_replica_count < current_replicas {
            // Use the provided list of active containers
            let mut sorted_containers = active_runpod_containers;
            sorted_containers.sort_by(|a, b| b.created_at.cmp(&a.created_at));

            // Remove containers from highest replica number down to new_replica_count
            let num_to_remove = (current_replicas - new_replica_count) as usize;
            debug!("Need to remove {} container(s)", num_to_remove);

            for container in sorted_containers.iter().take(num_to_remove)
            // Take the newest ones
            {
                info!(
                    "[Processor Controller] Removing container {} for processor {}",
                    container.name, processor.id
                );

                match platform.delete(&container.id, db).await {
                    Ok(_) => info!(
                        "[Processor Controller] Successfully removed container {} for processor {}",
                        container.name, processor.id
                    ),
                    Err(e) => error!(
                        "[Processor Controller] Failed to remove container {} for processor {}: {}",
                        container.name, processor.id, e
                    ),
                }

                // e) Remove the container record from DB
                debug!(
                    "Attempting to delete container DB record for ID: {}",
                    container.id
                );
                match crate::entities::containers::Entity::delete_by_id(container.id.clone()) // Clone ID for the call
                    .exec(db)
                    .await
                {
                    Ok(delete_result) => {
                        // delete_result is sea_orm::DeleteResult
                        if delete_result.rows_affected > 0 {
                            info!(
                                "Successfully deleted container DB record {} ({} row(s) affected).",
                                container.id, // Original container.id is fine here
                                delete_result.rows_affected
                            );
                        } else {
                            warn!(
                                "Container DB record {} not found or already deleted during attempt to delete.",
                                container.id
                            );
                        }
                    }
                    Err(e) => {
                        error!("Error deleting container DB record {}: {}", container.id, e);
                    }
                }
            }
        }

        Ok(())
    }
}

impl ProcessorPlatform for StandardProcessor {
    async fn declare(
        &self,
        config: &V1ProcessorRequest,
        db: &DatabaseConnection,
        user_profile: &V1UserProfile,
        owner_id: &str,
        namespace: &str,
    ) -> Result<V1Processor, Box<dyn std::error::Error + Send + Sync>> {
        // 1. Generate a unique ID for the new processor.
        let new_id = ShortUuid::generate().to_string();
        let name = config
            .metadata
            .name
            .clone()
            .unwrap_or(petname::petname(3, "-").unwrap());

        debug!(
            "Declaring processor {:?} in namespace {:?}",
            name, namespace
        );

        // 2. Create an ActiveModel to represent the new record in the database.
        let processor_am = processors::ActiveModel {
            // Primary fields
            id: Set(new_id),
            name: Set(name.clone()),
            namespace: Set(namespace.to_string()),
            full_name: Set(format!("{}/{}", namespace, name)),
            owner: Set(owner_id.to_string()),
            created_by: Set(Some(user_profile.email.clone())),

            // Any JSON fields from config (e.g., container & scale).
            // Adjust as needed depending on your actual request struct.
            container: Set(config
                .container
                .clone()
                .map(|c| serde_json::to_value(c))
                .transpose()?),
            scale: Set(
                config
                    .scale
                    .clone()
                    .map(serde_json::to_value)
                    .transpose()? // produces Result<Option<JsonValue>, _>
                    .unwrap_or(serde_json::Value::Null), // ensure a valid JSON Value
            ),
            labels: Set(config
                .metadata
                .labels
                .clone()
                .map(|l| serde_json::to_value(l))
                .transpose()?),

            stream: Set(format!("processor:{}:{}", namespace, name)),

            // Typically set an initial status or desired_status to "Defined" or similar.
            status: Set(Some(serde_json::to_value(V1ProcessorStatus {
                status: Some(ProcessorStatus::Defined.to_string()),
                message: None,
                pressure: None,
            })?)),
            desired_status: Set(Some(ProcessorStatus::Running.to_string())),

            // For scale, you might also set min_replicas/max_replicas if that's appropriate.
            min_replicas: Set(config.min_replicas.clone()),
            max_replicas: Set(config.max_replicas.clone()),

            // Auto-set timestamps.
            created_at: Set(Utc::now().into()),
            updated_at: Set(Utc::now().into()),

            ..Default::default()
        };

        debug!("Processor ActiveModel: {:?}", processor_am);

        // 3. Insert into the DB.
        let inserted_model = match processor_am.insert(db).await {
            Ok(model) => model,
            Err(e) => {
                error!("Error inserting processor {:?}: {:?}", name, e);
                return Err(e.into());
            }
        };

        debug!("Inserted processor: {:?}", inserted_model);

        // --- BEGIN: Add Processor Agent Key Creation ---
        debug!(
            "Creating agent key for processor {}",
            inserted_model.id.clone()
        );

        // Assume a function exists to create the key using user profile
        // We need the auth server URL, user token, desired agent ID, name, and duration.
        let client_config = ClientConfig::read()
            .map_err(|e| format!("Failed to read global config: {}", e))?;
        let auth_server = client_config
            .get_current_server_config()
            .and_then(|s| s.auth_server.clone())
            .expect("Auth server URL not configured");

        let user_token = user_profile
            .token
            .as_ref()
            .ok_or_else(|| "User profile token is missing".to_string())?;

        let agent_key_request = V1CreateAgentKeyRequest {
            agent_id: format!("processor-{}", inserted_model.id),
            name: format!("Processor Key for {}", inserted_model.name),
            duration: 31536000, // e.g., 1 year
        };

        let processor_agent_key_response =
            match create_agent_key(&auth_server, user_token, agent_key_request).await {
                Ok(response) => response,
                Err(e) => {
                    error!(
                        "Failed to create agent key for processor {}: {}",
                        inserted_model.id, e
                    );
                    return Err(format!("Failed to create agent key for processor: {}", e).into());
                }
            };

        let processor_agent_key = processor_agent_key_response
            .key
            .ok_or_else(|| "Auth server did not return an agent key".to_string())?;

        // Store the processor's agent key as a secret
        let secret_name = format!("processor-agent-key-{}", inserted_model.id);
        let secret_namespace = "root";
        let secret_full_name = format!("{}/{}", secret_namespace, secret_name);

        // --- BEGIN: Check and Delete Existing Secret ---
        debug!(
            "Checking for existing secret {}/{}",
            secret_namespace, secret_name
        );
        match Query::find_secret_by_namespace_and_name(db, secret_namespace, &secret_name).await {
            Ok(Some(existing_secret)) => {
                info!(
                    "Found existing secret {}/{}, deleting it before creating a new one.",
                    secret_namespace, secret_name
                );
                match crate::entities::secrets::Entity::delete_by_id(existing_secret.id)
                    .exec(db)
                    .await
                {
                    Ok(_) => debug!("Successfully deleted existing secret."),
                    Err(e) => {
                        error!(
                            "Failed to delete existing secret {}: {}",
                            secret_full_name, e
                        );
                        // Decide if we should return an error here or continue
                        return Err(format!("Failed to delete existing secret: {}", e).into());
                    }
                }
            }
            Ok(None) => {
                debug!("No existing secret found, proceeding with creation.");
            }
            Err(e) => {
                error!(
                    "Error checking for existing secret {}: {}",
                    secret_full_name, e
                );
                return Err(format!("Database error checking for existing secret: {}", e).into());
            }
        }
        // --- END: Check and Delete Existing Secret ---

        debug!("Storing processor agent key secret: {}", secret_full_name);
        // Adapt store_ssh_keypair logic for storing a single secret
        let secret_model = crate::entities::secrets::Model::new(
            ShortUuid::generate().to_string(), // Use a new UUID for the secret's own ID
            secret_name,                       // Name of the secret
            secret_namespace.to_string(),      // Namespace for the secret
            user_profile.email.clone(),        // User who created/owns this secret record
            &processor_agent_key,              // The value to encrypt and store
            Some(inserted_model.id.clone()),   // owner_ref links to the processor
            None,                              // Labels
            None,                              // Expires_at
        )
        .map_err(|e| format!("Failed to prepare secret model: {}", e))?;

        let active_secret_model: crate::entities::secrets::ActiveModel = secret_model.into();

        crate::entities::secrets::Entity::insert(active_secret_model)
            .exec(db)
            .await
            .map_err(|e| {
                error!(
                    "Failed to store processor agent key secret {}: {}",
                    secret_full_name, e
                );
                format!("Failed to store processor agent key secret: {}", e)
            })?;

        // Update the processor record with the secret ID

        let v1_processor = match inserted_model.to_v1_processor() {
            Ok(processor) => processor,
            Err(e) => {
                error!(
                    "Error converting processor {:?} to V1Processor: {:?}",
                    name, e
                );
                return Err(e.into());
            }
        };

        debug!("V1 processor: {:?}", v1_processor);

        Ok(v1_processor)
    }

    async fn reconcile(
        &self,
        processor: &processors::Model,
        db: &DatabaseConnection,
        redis_client: &redis::Client,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        debug!(
            "[DEBUG:standard.rs:reconcile] Entering reconcile for processor {}",
            processor.id
        );

        // --- BEGIN: Get Processor's Agent Key and User Profile ---
        let secret_name = format!("processor-agent-key-{}", processor.id);
        let secret_namespace = "root"; // As defined in `declare`

        debug!("Fetching secret {}/{}", secret_namespace, secret_name);
        let secret_model =
            Query::find_secret_by_namespace_and_name(db, secret_namespace, &secret_name)
                .await
                .map_err(|e| format!("Database error fetching secret: {}", e))?
                .ok_or_else(|| {
                    format!(
                        "Secret '{}/{}' not found for processor {}",
                        secret_namespace, secret_name, processor.id
                    )
                })?;

        debug!("Decrypting secret value for processor {}", processor.id);
        let agent_key = secret_model
            .decrypt_value()
            .map_err(|e| format!("Failed to decrypt agent key: {}", e))?;

        let client_config = ClientConfig::read()
            .map_err(|e| format!("Failed to read global config: {}", e))?;
        let auth_server = client_config
            .get_current_server_config()
            .and_then(|s| s.auth_server.clone())
            .expect("Auth server URL not configured");

        debug!(
            "Fetching user profile using processor agent key from {}",
            auth_server
        );
        let client = reqwest::Client::new();
        let user_profile_url = format!("{}/v1/users/me", auth_server);

        let response = client
            .get(&user_profile_url)
            .header("Authorization", format!("Bearer {}", agent_key))
            .send()
            .await
            .map_err(|e| format!("Auth request to {} failed: {}", user_profile_url, e))?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Failed to read error body".to_string());
            error!("Auth request failed with status {}: {}", status, error_text);
            return Err(
                format!("Auth request failed with status {}: {}", status, error_text).into(),
            );
        }

        let owner_profile = response
            .json::<V1UserProfile>()
            .await
            .map_err(|e| format!("Failed to parse user profile response: {}", e))?;

        debug!("Retrieved owner profile: {:?}", owner_profile);

        // 1) Attempt to parse the current status from the DB row
        if let Ok(Some(parsed_status)) = processor.parse_status() {
            let status_str = parsed_status
                .status
                .clone()
                .unwrap_or_else(|| ProcessorStatus::Invalid.to_string());
            debug!(
                "[DEBUG:standard.rs:reconcile] Processor {} has status '{}'",
                processor.id, status_str
            );

            let status = ProcessorStatus::from_str(&status_str).unwrap_or(ProcessorStatus::Invalid);

            // 2) If it needs to start, call our 'start_processor' helper
            if status.needs_start() {
                info!(
                    "[Processor Controller] Processor {} needs to be started",
                    processor.id
                );
                if let Some(ds) = &processor.desired_status {
                    if ds == &ProcessorStatus::Running.to_string() {
                        info!(
                            "[Processor Controller] Processor {} desired_status is 'Running'; starting...",
                            processor.id
                        );
                        self.start_processor(db, processor.clone(), &owner_profile, redis_client)
                            .await?;
                    } else {
                        info!(
                            "[Processor Controller] Processor {} desired_status is '{}', not 'Running'",
                            processor.id, ds
                        );
                    }
                } else {
                    info!(
                        "[Processor Controller] Processor {} has no desired_status. Skipping start.",
                        processor.id
                    );
                }
            }

            // 3) If it needs to be watched, we call our watch helper
            if status.needs_watch() {
                info!(
                    "[Processor Controller] Processor {} needs to be watched",
                    processor.id
                );
                // 1) Match on the enum to get the Redis Client, if it's a Redis-based queue
                match &self.state.message_queue {
                    MessageQueue::Redis { client } => {
                        self.watch_processor(
                            db,
                            processor.clone(),
                            &owner_profile,
                            client.as_ref(),
                        )
                        .await?;
                    }
                    MessageQueue::Kafka { .. } => {
                        info!("[Processor Controller] Not a Redis queue; skipping watch");
                    }
                }
            }
        } else {
            warn!(
                "[Processor Controller] Processor {} has no parsable status; skipping reconcile",
                processor.id
            );
        }

        debug!(
            "[DEBUG:standard.rs:reconcile] Completed reconcile for processor {}",
            processor.id
        );
        Ok(())
    }

    async fn delete(
        &self,
        id: &str,
        db: &DatabaseConnection,
        redis_client: &redis::Client,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        debug!("Deleting processor: {}", id);
        use crate::entities::processors;
        use crate::query::Query;
        use crate::resources::v1::containers::factory::platform_factory;
        use sea_orm::EntityTrait;

        debug!("Finding processor: {}", id);
        // 1) Find the processor in the database by `id`.
        let Some(processor) = processors::Entity::find_by_id(id.to_string())
            .one(db)
            .await?
        else {
            return Ok(());
        };

        tracing::info!("Deleting processor '{}'...", processor.id);

        // --- BEGIN: Set desired state to terminate ---
        let mut processor_active_model: processors::ActiveModel = processor.clone().into();
        processor_active_model.desired_replicas = Set(Some(0));
        processor_active_model.desired_status = Set(Some(ProcessorStatus::Stopped.to_string()));
        // Ignore errors here? If DB update fails, deletion might be problematic anyway.
        // Let's log error but continue the delete process.
        if let Err(e) = processor_active_model.update(db).await {
            error!("Failed to update processor {} desired state during deletion: {}. Continuing deletion...", processor.id, e);
        }
        // --- END: Set desired state to terminate ---

        // --- BEGIN: Delete Redis Stream ---
        let stream_name = processor.stream.clone();
        debug!(
            "Attempting to delete Redis stream '{}' for processor {}",
            stream_name, processor.id
        );
        match redis_client.get_connection() {
            Ok(mut conn) => {
                match redis::cmd("DEL").arg(&stream_name).query::<()>(&mut conn) {
                    Ok(_) => info!(
                        "Successfully deleted Redis stream '{}' for processor {}",
                        stream_name, processor.id
                    ),
                    Err(e) => error!(
                        "Failed to delete Redis stream '{}' for processor {}: {}",
                        stream_name,
                        processor.id,
                        e // Decide if this should be a hard error or just logged
                    ),
                }
            }
            Err(e) => {
                error!(
                    "Failed to get Redis connection to delete stream '{}' for processor {}: {}",
                    stream_name,
                    processor.id,
                    e // Decide if this should be a hard error or just logged
                );
            }
        }
        // --- END: Delete Redis Stream ---

        // 2) Query containers using the correct owner_ref format
        let owner_ref_string = format!("{}.{}.Processor", processor.name, processor.namespace);
        let associated_containers_result =
            Query::find_containers_by_owner_ref(db, &owner_ref_string).await; // Use the formatted string
        debug!(
            "Container query result for processor {} using owner_ref '{}': {:?}",
            processor.id, owner_ref_string, associated_containers_result
        );
        let associated_containers = associated_containers_result?;

        debug!(
            "Found {} containers referencing processor '{}'",
            associated_containers.len(),
            processor.id
        );
        if associated_containers.is_empty() {
            tracing::info!(
                "No containers found referencing processor '{}'",
                processor.id
            );
        } else {
            tracing::info!(
                "Found {} container(s) referencing processor '{}'",
                associated_containers.len(),
                processor.id
            );
        }

        // 3) We'll remove each container from its own platform:
        for container in associated_containers {
            debug!("Deleting container: {}", container.id);
            // a) Parse the container's intended platform (e.g. "runpod" or "kube")
            let platform_str = container.platform.clone().unwrap_or("runpod".to_string());
            // fallback to "runpod" or whichever makes sense
            debug!("Platform string: {}", platform_str);
            let platform = platform_factory(platform_str);
            match platform.delete(&container.id, db).await {
                Ok(_) => info!(
                    "Successfully deleted container {} from platform",
                    container.id
                ),
                Err(e) => error!(
                    "Failed to delete container {} from platform: {}",
                    container.id, e
                ),
            }

            // e) Remove the container record from DB
            debug!(
                "Attempting to delete container DB record for ID: {}",
                container.id
            );
            match crate::entities::containers::Entity::delete_by_id(container.id.clone()) // Clone ID for the call
                .exec(db)
                .await
            {
                Ok(delete_result) => {
                    // delete_result is sea_orm::DeleteResult
                    if delete_result.rows_affected > 0 {
                        info!(
                            "Successfully deleted container DB record {} ({} row(s) affected).",
                            container.id, // Original container.id is fine here
                            delete_result.rows_affected
                        );
                    } else {
                        warn!(
                            "Container DB record {} not found or already deleted during attempt to delete.",
                            container.id
                        );
                    }
                }
                Err(e) => {
                    error!("Error deleting container DB record {}: {}", container.id, e);
                }
            }
        }

        // --- BEGIN: Delete Associated Secret ---
        let secret_name = format!("processor-agent-key-{}", processor.id);
        let secret_namespace = "root"; // As defined in `declare`
        debug!(
            "Attempting to delete secret {}/{} for processor {}",
            secret_namespace, secret_name, processor.id
        );

        match Query::find_secret_by_namespace_and_name(db, secret_namespace, &secret_name).await {
            Ok(Some(secret_model)) => {
                match crate::entities::secrets::Entity::delete_by_id(secret_model.id)
                    .exec(db)
                    .await
                {
                    Ok(_) => info!(
                        "Successfully deleted secret {}/{} for processor {}",
                        secret_namespace, secret_name, processor.id
                    ),
                    Err(e) => error!(
                        "Failed to delete secret {}/{} for processor {}: {}",
                        secret_namespace, secret_name, processor.id, e
                    ),
                }
            }
            Ok(None) => {
                info!(
                    "Secret {}/{} not found for processor {}, skipping deletion.",
                    secret_namespace, secret_name, processor.id
                );
            }
            Err(e) => {
                error!(
                    "Error finding secret {}/{} for processor {}: {}",
                    secret_namespace, secret_name, processor.id, e
                );
                // Decide if this should be a hard error or just logged
            }
        }
        // --- END: Delete Associated Secret ---

        debug!("Deleting processor record: {}", processor.id);
        // 4) Finally, delete the processor record
        processors::Entity::delete_by_id(processor.id)
            .exec(db)
            .await?;
        tracing::info!(
            "Successfully deleted processor '{}' and its associated containers.",
            id
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[allow(unused_imports)]
    use super::*;

    // Unit tests for StandardProcessor
    #[tokio::test]
    async fn test_declare() {
        // Test implementation
    }

    #[tokio::test]
    async fn test_reconcile() {
        // Test implementation
    }

    #[tokio::test]
    async fn test_delete() {
        // Test implementation
    }
}
