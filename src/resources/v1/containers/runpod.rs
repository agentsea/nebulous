use crate::accelerator::base::AcceleratorProvider;
use crate::accelerator::runpod::RunPodProvider;
use crate::config::SERVER_CONFIG;
use crate::entities::containers;
use crate::models::{V1Meter, V1UserProfile};
use crate::mutation::{self, Mutation};
use crate::oci::client::pull_and_parse_config;
use crate::query::Query;
use crate::resources::v1::containers::base::{ContainerPlatform, ContainerStatus};
use crate::resources::v1::containers::models::{
    RestartPolicy, V1Container, V1ContainerHealthCheck, V1ContainerRequest, V1ContainerResources,
    V1ContainerStatus, V1EnvVar, V1Port, V1UpdateContainer,
};
use crate::resources::v1::volumes::models::V1VolumePath;
use crate::ssh::exec::run_ssh_command_ts;
use crate::ssh::keys;
use crate::volumes::rclone::{SymlinkConfig, VolumeConfig, VolumePath};
use petname;
use regex::Regex;
use runpod::*;
use sea_orm::{ActiveModelTrait, DatabaseConnection, Set};
use short_uuid::ShortUuid;
use std::collections::HashMap;
use std::str::FromStr;
use std::time::Duration;
use tracing::{debug, error, info, warn};

/// A `TrainingPlatform` implementation that schedules training jobs on RunPod.
#[derive(Clone)]
pub struct RunpodPlatform {
    runpod_client: RunpodClient,
}

impl RunpodPlatform {
    pub fn new() -> Self {
        // Read the API key from environment variables
        let api_key = std::env::var("RUNPOD_API_KEY")
            .expect("[Runpod Controller] Missing RUNPOD_API_KEY environment variable");

        RunpodPlatform {
            runpod_client: RunpodClient::new(api_key),
        }
    }

    /// Create a new RunpodPlatform with a specific API key
    pub fn with_api_key(api_key: String) -> Self {
        RunpodPlatform {
            runpod_client: RunpodClient::new(api_key),
        }
    }

    /// Report metrics to OpenMeter for a running container
    async fn report_meters(
        &self,
        container_id: String,
        seconds: u64,
        meters: &serde_json::Value,
        owner_id: String,
        base_cost_per_hr: Option<f64>,
        gpu_type: String,
    ) {
        // Parse the meters from the container model
        let meters_vec: Vec<V1Meter> = match serde_json::from_value(meters.clone()) {
            Ok(parsed) => parsed,
            Err(e) => {
                error!("[Runpod Controller] Failed to parse meters: {}", e);
                return;
            }
        };

        if meters_vec.is_empty() {
            return;
        }

        // Get OpenMeter configuration from environment
        let openmeter_url = match std::env::var("OPENMETER_URL") {
            Ok(url) => url,
            Err(_) => {
                error!("[Runpod Controller] OPENMETER_URL environment variable not set");
                return;
            }
        };

        let openmeter_token = match std::env::var("OPENMETER_TOKEN") {
            Ok(token) => token,
            Err(_) => {
                error!("[Runpod Controller] OPENMETER_TOKEN environment variable not set");
                return;
            }
        };

        // Create OpenMeter client
        let meter_client = openmeter::MeterClient::new(openmeter_url, openmeter_token);

        // Create and send events for each meter
        for meter in meters_vec {
            // Determine final cost using cost plus cost percentage if present.
            let mut cost_value = if let Some(costp) = meter.costp {
                // If costp is specified (percentage field), we need a base cost to add to.
                if let Some(base_cost) = base_cost_per_hr {
                    if base_cost == 0.0 {
                        // Print a warning if the base cost is zero
                        warn!(
                        "[Runpod Controller] cost=0.0 but costp={}% was supplied for metric '{}'. Final cost would still be 0.0.",
                        costp, meter.metric
                    );
                    }
                    // e.g., if user sets cost=1.0 and costp=10.0, final cost=1.1
                    base_cost + (base_cost * costp / 100.0)
                } else {
                    // If no base cost is provided but costp is provided,
                    // treat it as "percentage of 0" -> 0
                    warn!(
                    "[Runpod Controller] costp={}% provided but cost=None for metric '{}', using 0",
                    costp, meter.metric
                );
                    0.0
                }
            } else if let Some(c) = meter.cost {
                // cost provided, no costp
                c
            } else {
                // if neither cost nor costp is present, log a warning and skip this meter
                warn!(
                    "[Runpod Controller] No cost or costp provided for meter '{}', skipping.",
                    meter.metric
                );
                continue;
            };

            // 2) We now have a base cost_value which is implicitly "per hour".
            //    But the meter could be in seconds, minutes, or hours. Adjust cost_value accordingly:
            // If costp is present, interpret cost_value as "per hour" and adjust according to the unit
            if meter.costp.is_some() {
                match meter.unit.to_lowercase().as_str() {
                    "second" | "seconds" => {
                        // cost_value is "per hour", so for 1 second it's cost_value / 3600
                        cost_value /= 3600.0;
                    }
                    "minute" | "minutes" => {
                        // cost_value is "per hour", so for 1 minute it's cost_value / 60
                        cost_value /= 60.0;
                    }
                    "hour" | "hours" => {
                        // It's already "per hour", so no change needed
                    }
                    other => {
                        warn!(
                            "[Runpod Controller] Unknown meter.unit='{}' for metric '{}', skipping.",
                            other, meter.metric
                        );
                        continue;
                    }
                }
            }

            let event_id = format!("container-{}-{}", container_id, uuid::Uuid::new_v4());

            // Create event data based on meter type
            let data = match meter.metric.as_str() {
                "runtime" => {
                    // For runtime, we'll report 1 unit (e.g., 1 minute of runtime)
                    serde_json::json!({
                        "value": seconds as f64,
                        "metric": meter.metric,
                        "container_id": container_id,
                        "currency": meter.currency,
                        "cost": cost_value,
                        "unit": meter.unit,
                        "kind": "Container",
                        "service": "Nebulous",
                        "gpu_type": gpu_type,
                    })
                }
                // Add other metric types as needed
                _ => {
                    serde_json::json!({
                        "value": seconds as f64,
                        "metric": meter.metric,
                        "container_id": container_id,
                        "currency": meter.currency,
                        "cost": cost_value,
                        "unit": meter.unit,
                        "kind": "Container",
                        "service": "Nebulous",
                        "gpu_type": gpu_type,
                    })
                }
            };

            // Create CloudEvent
            let cloud_event = openmeter::CloudEvent {
                id: event_id,
                source: "nebulous-runpod-controller".to_string(),
                specversion: "1.0".to_string(),
                r#type: meter.metric.clone(),
                subject: owner_id.clone(),
                time: Some(chrono::Utc::now().to_rfc3339()),
                dataschema: None,
                datacontenttype: Some("application/json".to_string()),
                data: Some(data),
            };

            // Send the event to OpenMeter
            match meter_client.ingest_events(&[cloud_event]).await {
                Ok(_) => {
                    debug!(
                        "[Runpod Controller] Successfully reported meter {:?} for container {}",
                        meter, container_id
                    );
                }
                Err(e) => {
                    error!(
                        "[Runpod Controller] Failed to report meter {:?} for container {}: {}",
                        meter, container_id, e
                    );
                }
            }
        }
    }

    pub async fn get_public_ports_for_pod(
        &self,
        pod_id: &str,
    ) -> Result<Vec<V1Port>, Box<dyn std::error::Error + Send + Sync>> {
        // Fetch all pods (with their ports) from RunPod
        let pods_with_ports_data = self.runpod_client.fetch_my_pods_with_ports().await?;

        // Find the matching pod by ID
        let maybe_pod = pods_with_ports_data
            .data
            .unwrap()
            .myself
            .pods
            .into_iter()
            .find(|p| p.id == pod_id);

        // If the pod was found, extract its public ports as Vec<V1Port>
        if let Some(pod) = maybe_pod {
            // The `runtime` field is optional, so check if it's present
            if let Some(runtime) = pod.runtime {
                // Filter only the public ports, then map fields into our V1Port model
                let public_ports: Vec<V1Port> = runtime
                    .ports
                    .into_iter()
                    .filter(|port| port.is_ip_public)
                    .map(|port| V1Port {
                        port: port.public_port as u16,
                        protocol: Some("tcp".to_string()),
                        public_ip: Some(port.ip),
                    })
                    .collect();

                Ok(public_ports)
            } else {
                // If there's no runtime info, return an empty list
                Ok(vec![])
            }
        } else {
            // If we couldn't find a pod with this ID, return an error
            Err(format!("No pod found for ID: {}", pod_id).into())
        }
    }

    /// Watch a pod and update its status in the database
    pub async fn watch(
        &self,
        db: &DatabaseConnection,
        container: containers::Model,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!(
            "[Runpod Controller] Starting to watch pod for container {}",
            container.id
        );
        let container_id = container.id.to_string();
        let pause_seconds = 5;
        let duration = Duration::from_secs(pause_seconds);

        // Parse timeout if specified
        let timeout_duration = if let Some(timeout_str) = &container.timeout {
            match humantime::parse_duration(timeout_str) {
                Ok(timeout) => {
                    info!(
                        "[Runpod Controller] Container {} has timeout of {:?}",
                        container_id, timeout
                    );
                    Some((timeout, timeout_str.clone()))
                }
                Err(e) => {
                    error!(
                        "[Runpod Controller] Failed to parse timeout '{}' for container {}: {}",
                        timeout_str, container_id, e
                    );
                    None
                }
            }
        } else {
            None
        };

        // Track container start time for timeout calculation
        let mut container_start_time: Option<std::time::Instant> = None;

        // Get initial status from database
        let (mut last_status, resource_name) =
            match crate::query::Query::find_container_by_id(db, container_id.clone().to_string())
                .await
            {
                Ok(container) => {
                    info!(
                        "[Runpod Controller] Initial database container for {}: {:?}",
                        container_id.clone(),
                        container
                    );

                    // Fix: Parse the status JSON properly
                    let status =
                        container
                            .as_ref()
                            .and_then(|c| c.status.clone())
                            .and_then(|status_json| {
                                // Try to extract the status field from the JSON
                                if let Some(status_obj) = status_json.as_object() {
                                    if let Some(status_value) = status_obj.get("status") {
                                        if let Some(status_str) = status_value.as_str() {
                                            return Some(
                                                ContainerStatus::from_str(status_str)
                                                    .unwrap_or(ContainerStatus::Pending),
                                            );
                                        }
                                    }
                                }
                                None
                            });

                    let resource_name = container.as_ref().and_then(|c| c.resource_name.clone());

                    (status, resource_name)
                }
                Err(e) => {
                    error!(
                        "[Runpod Controller] Error fetching initial container from database: {}",
                        e
                    );
                    (None, None)
                }
            };

        info!("[Runpod Controller] Resource name: {:?}", resource_name);
        // Use resource_name from database if available, otherwise use the provided pod_id
        let pod_id_to_watch = resource_name.clone().unwrap_or_default();
        info!(
            "[Runpod Controller] Using pod ID for watching: {}",
            pod_id_to_watch
        );

        let mut consecutive_errors = 0;
        const MAX_ERRORS: usize = 5;

        // Poll the pod status every 20 seconds
        let mut iteration_count = 0;
        loop {
            iteration_count += 1;
            debug!(
                "[DEBUG:runpod.rs:watch] container={} iteration={}",
                container_id.clone(),
                iteration_count
            );

            match self.runpod_client.get_pod(&pod_id_to_watch).await {
                Ok(pod_response) => {
                    debug!(
                        "[DEBUG:runpod.rs:watch] container={} got pod_response: {:?}",
                        container_id.clone(),
                        pod_response
                    );
                    consecutive_errors = 0;

                    if let Some(pod_info) = pod_response.data {
                        debug!("[Runpod Controller] response data present");

                        match Mutation::update_container_status(
                            db,
                            container_id.to_string(),
                            None,
                            None,
                            None,
                            None,
                            None,
                            Some(pod_info.cost_per_hr),
                            None,
                        )
                        .await
                        {
                            Ok(_) => {
                                info!("[Runpod Controller] Updated container status with cost_per_hr: {}", pod_info.cost_per_hr);
                            }
                            Err(e) => {
                                error!(
                                    "[Runpod Controller] Failed to update container status: {}",
                                    e
                                );
                            }
                        }

                        let ports = match self.get_public_ports_for_pod(&pod_id_to_watch).await {
                            Ok(ports) => ports,
                            Err(e) => {
                                error!(
                                    "[Runpod Controller] Error fetching ports for my pods: {}",
                                    e
                                );
                                vec![]
                            }
                        };
                        // Extract status information using desired_status field
                        let mut current_status = match pod_info.desired_status.as_str() {
                            "RUNNING" => ContainerStatus::Running,
                            "EXITED" => ContainerStatus::Completed,
                            "TERMINATED" => ContainerStatus::Stopped,
                            "DEAD" => ContainerStatus::Failed,
                            "CREATED" => ContainerStatus::Defined,
                            "RESTARTING" => ContainerStatus::Restarting,
                            "PAUSED" => ContainerStatus::Paused,
                            _ => {
                                info!(
                                    "[Runpod Controller] Unknown pod status: {}, defaulting to Pending",
                                    pod_info.desired_status
                                );
                                ContainerStatus::Pending
                            }
                        };
                        debug!("[Runpod Controller] current_status: {:?}", current_status);

                        let is_ssh_accessible = match self.is_ssh_accessible(&container).await {
                            Ok(is_accessible) => is_accessible,
                            Err(e) => {
                                error!(
                                    "[Runpod Controller] Error checking SSH accessibility: {}",
                                    e
                                );
                                false
                            }
                        };
                        if !is_ssh_accessible {
                            info!("[Runpod Controller] SSH is not accessible, setting status to Creating");
                            current_status = ContainerStatus::Creating;
                        }

                        match container.parse_health_check() {
                            Ok(Some(health_check)) => {
                                info!("[Runpod Controller] Health check: {:?}", health_check);
                                if let Err(e) = self
                                    .perform_health_check(
                                        &container,
                                        &health_check,
                                        &current_status,
                                        db,
                                    )
                                    .await
                                {
                                    error!("[Runpod Controller] Health check error: {}", e);
                                }
                            }
                            Ok(None) => {
                                info!("[Runpod Controller] No health check defined, using default status");
                            }
                            Err(e) => {
                                error!("[Runpod Controller] Failed to parse health check: {}", e);
                            }
                        }

                        // Handle metering if container has meters defined and status is Running
                        if current_status == ContainerStatus::Running {
                            // Start timing if this is the first time we've seen the container running
                            if container_start_time.is_none() {
                                info!("[Runpod Controller] Container {} started running, recording start time", container_id);
                                container_start_time = Some(std::time::Instant::now());
                            }

                            // Check timeout if applicable
                            if let (Some(start_time), Some(ref timeout_data)) =
                                (container_start_time, &timeout_duration)
                            {
                                let (timeout, timeout_str) = timeout_data;
                                let elapsed = start_time.elapsed();
                                if elapsed >= *timeout {
                                    info!(
                                        "[Runpod Controller] Container {} has exceeded timeout of {:?} (elapsed: {:?}), terminating",
                                        container_id, timeout, elapsed
                                    );

                                    // Terminate the container due to timeout
                                    if let Err(del_err) = self.delete(&container_id, db).await {
                                        error!(
                                            "[Runpod Controller] Error deleting timed-out container {}: {}",
                                            container_id, del_err
                                        );
                                    } else {
                                        // Update container status to indicate timeout
                                        if let Err(e) = Mutation::update_container_status(
                                            db,
                                            container_id.to_string(),
                                            Some(ContainerStatus::Failed.to_string()),
                                            Some(format!("Container terminated after exceeding timeout of {}", timeout_str)),
                                            None,
                                            None,
                                            None,
                                            None,
                                            None,
                                        )
                                        .await
                                        {
                                            error!(
                                                "[Runpod Controller] Failed to update status for timed-out container: {}",
                                                e
                                            );
                                        }
                                    }
                                    break;
                                } else {
                                    debug!(
                                        "[Runpod Controller] Container {} running for {:?} of {:?} timeout",
                                        container_id, elapsed, timeout
                                    );
                                }
                            }

                            // 1) Check for /done.txt in the container
                            info!("[Runpod Controller] container running...");

                            match self.get_tailscale_device_ip(&container).await {
                                Ok(ip) => {
                                    info!("[Runpod Controller] Acquired Tailscale IP: {}", ip);
                                    if let Err(e) = Mutation::update_container_tailnet_ip(
                                        db,
                                        container_id.clone(),
                                        ip.clone(),
                                    )
                                    .await
                                    {
                                        error!(
                                            "[Runpod Controller] Failed to update container tailnet IP for {}: {}",
                                            container_id, e
                                        );
                                    }
                                }
                                Err(e) => {
                                    warn!(
                                        "[Runpod Controller] Failed to get Tailscale device IP for container {}: {}",
                                        container.id, e
                                    );
                                }
                            };
                            info!(
                                "[Runpod Controller] container.restart={}",
                                container.restart
                            );
                            if container.restart.to_lowercase()
                                == RestartPolicy::Never.to_string().to_lowercase()
                            {
                                info!("[Runpod Controller] checking for /done.txt");
                                match self
                                    .check_done_file(
                                        &container_id,
                                        &container
                                            .container_user
                                            .clone()
                                            .unwrap_or("root".to_string()),
                                        db,
                                    )
                                    .await
                                {
                                    Ok(true) => {
                                        info!(
                                    "[Runpod Controller] /done.txt found for container {} -> deleting container",
                                    container_id
                                );
                                        if let Err(del_err) = self.delete(&container_id, db).await {
                                            error!(
                                            "[Runpod Controller] Error deleting container {}: {}",
                                            container_id, del_err
                                        );
                                        }
                                        // Once deleted, no further watch is needed
                                        break;
                                    }
                                    Ok(false) => {
                                        // Not done yet, keep going
                                    }
                                    Err(check_err) => {
                                        error!(
                                    "[Runpod Controller] Error checking done file on container {}: {}",
                                    container_id, check_err
                                );
                                    }
                                }
                            }
                            if let Some(meters) = &container.meters {
                                info!(
                                    "[Runpod Controller] reporting meters for container {}",
                                    container_id
                                );

                                let gpu_type = match container.parse_status() {
                                    Ok(Some(status)) => {
                                        status.accelerator.unwrap_or("Unknown".to_string())
                                    }
                                    Ok(None) => "Unknown".to_string(),
                                    Err(_) => "Unknown".to_string(),
                                };
                                if gpu_type == "Unknown" {
                                    warn!("[Runpod Controller] Could not find accelerator in status setting to 'Unknown'");
                                }
                                self.report_meters(
                                    container_id.clone(),
                                    pause_seconds,
                                    meters,
                                    container.owner.clone(),
                                    Some(pod_info.cost_per_hr),
                                    gpu_type,
                                )
                                .await;
                            }
                        }

                        info!(
                            "[Runpod Controller] Current RunPod status: {}",
                            current_status
                        );
                        info!(
                            "[Runpod Controller] Last database status: {:?}",
                            last_status
                        );

                        // If status changed, update the database
                        if last_status.as_ref() != Some(&current_status) {
                            if let Some(last) = &last_status {
                                info!(
                                    "[Runpod Controller] Pod {:?} status changed: {} -> {}",
                                    resource_name.clone(),
                                    last,
                                    current_status
                                );
                            } else {
                                info!(
                                    "[Runpod Controller] Pod {:?} initial status: {}",
                                    resource_name, current_status
                                );
                            }

                            // Update the database with the new status using the Mutation struct
                            match crate::mutation::Mutation::update_container_status(
                                db,
                                container_id.to_string(),
                                Some(current_status.to_string()),
                                None,
                                None,
                                Some(ports),
                                None,
                                None,
                                None,
                            )
                            .await
                            {
                                Ok(_) => {
                                    info!(
                                        "[Runpod Controller] Updated container {:?} status to {}",
                                        container_id, current_status
                                    );
                                    // Update last_status after successful database update
                                    last_status = Some(current_status.clone());
                                }
                                Err(e) => {
                                    error!(
                                    "[Runpod Controller] Failed to update container status in database: {}",
                                    e
                                )
                                }
                            }

                            // If the pod is in a terminal state, exit the loop
                            if current_status.is_inactive() {
                                info!(
                                    "[Runpod Controller] Pod {:?} reached terminal state: {}",
                                    resource_name, current_status
                                );
                                break;
                            }
                        }
                    } else {
                        error!(
                            "[Runpod Controller] No pod data returned for pod {:?}",
                            resource_name
                        );

                        // Check if pod was deleted or doesn't exist
                        match self.runpod_client.list_pods().await {
                            Ok(pods_list) => {
                                if let Some(my_pods) = pods_list.data {
                                    info!("[Runpod Controller] My pods: {:?}", my_pods);
                                    if !my_pods
                                        .pods
                                        .iter()
                                        .any(|p| &p.id == resource_name.as_ref().unwrap())
                                    {
                                        info!(
                                            "[Runpod Controller] Pod {:?} no longer exists, marking job as completed",
                                            resource_name
                                        );

                                        // Update job as failed in database
                                        if let Some(ref current_status) = last_status {
                                            // If current status is non-terminal, allow an update to Completed
                                            if !current_status.is_inactive() {
                                                if let Err(e) = crate::mutation::Mutation::update_container_status(
                                                    db,
                                                    container_id.to_string(),
                                                    Some(ContainerStatus::Completed.to_string()),
                                                    Some("Pod no longer exists".to_string()),
                                                    None,
                                                    None,
                                                    None,
                                                    None,
                                                    None,
                                                )
                                                .await
                                                {
                                                    error!(
                                                        "[Runpod Controller] Failed to update job status in database: {}",
                                                        e
                                                    );
                                                }
                                            } else {
                                                // Already in a terminal state—don't overwrite it
                                                info!(
                                                    "[Runpod Controller] Container {} is already in terminal state ({:?}); not updating.",
                                                    container_id, current_status
                                                );
                                            }
                                        } else {
                                            // If we have no prior status, treat it as if it's not terminal
                                            if let Err(e) =
                                                crate::mutation::Mutation::update_container_status(
                                                    db,
                                                    container_id.to_string(),
                                                    Some(ContainerStatus::Completed.to_string()),
                                                    Some("Pod no longer exists".to_string()),
                                                    None,
                                                    None,
                                                    None,
                                                    None,
                                                    None,
                                                )
                                                .await
                                            {
                                                error!(
                                                    "[Runpod Controller] Failed to update job status in database: {}",
                                                    e
                                                );
                                            }
                                        }

                                        break;
                                    }
                                }
                            }
                            Err(e) => error!("[Runpod Controller] Error listing pods: {}", e),
                        }
                    }
                }
                // ------------------------------------------------
                // handle 404 not found
                // ------------------------------------------------
                Err(e) if is_not_found(&e) => {
                    error!(
                        "[Runpod Controller] Pod {} not found (404). Assuming it is deleted.",
                        pod_id_to_watch
                    );

                    // Check if container still exists in DB
                    match crate::query::Query::find_container_by_id(db, container_id.to_string())
                        .await
                    {
                        Ok(Some(_container)) => {
                            // If the container row is still there, mark it "Stopped" or "Deleted"
                            error!(
                                "[Runpod Controller] Marking container {} as stopped",
                                container_id
                            );
                            if let Err(update_err) =
                                crate::mutation::Mutation::update_container_status(
                                    db,
                                    container_id.to_string(),
                                    Some(ContainerStatus::Stopped.to_string()),
                                    Some("Pod was deleted or does not exist.".to_string()),
                                    None,
                                    None,
                                    None,
                                    None,
                                    None,
                                )
                                .await
                            {
                                error!(
                                    "[Runpod Controller] Failed to update container status in database: {}",
                                    update_err
                                );
                            }
                        }
                        Ok(None) => {
                            info!(
                                "[Runpod Controller] Container {} not found in DB (already removed). Nothing to update.",
                                container_id
                            );
                        }
                        Err(db_err) => {
                            error!(
                                "[Runpod Controller] Failed checking DB for container {}: {}",
                                container_id, db_err
                            );
                        }
                    }

                    // Either way, we can exit this watch since the RunPod resource does not exist
                    break;
                }
                Err(e) => {
                    error!("[Runpod Controller] Error fetching pods status: {}", e);
                    consecutive_errors += 1;

                    // If we've had too many consecutive errors, mark the job as failed
                    if consecutive_errors >= MAX_ERRORS {
                        error!(
                            "[Runpod Controller] Too many consecutive errors, marking container {} as failed",
                            container_id
                        );

                        if let Err(e) = crate::mutation::Mutation::update_container_status(
                            db,
                            container_id.to_string(),
                            Some(ContainerStatus::Failed.to_string()),
                            Some("Too many consecutive errors".to_string()),
                            None,
                            None,
                            None,
                            None,
                            None,
                        )
                        .await
                        {
                            error!(
                                "[Runpod Controller] Failed to update container status in database: {}",
                                e
                            );
                        }

                        break;
                    }
                }
            }
            debug!(
                "[DEBUG:runpod.rs:watch] container={} iteration={} sleeping 20s",
                container_id, iteration_count
            );
            // Wait before checking again
            tokio::time::sleep(duration).await;
        }

        // Unreachable if loop never breaks. If you do break eventually:
        debug!(
            "[DEBUG:runpod.rs:watch] Exiting watch for container {}",
            container_id
        );
        Ok(())
    }

    /// Check if the container is accessible via SSH
    async fn is_ssh_accessible(
        &self,
        container: &containers::Model,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let hostname = match &container.tailnet_ip {
            Some(ip) => ip.clone(),
            None => self.get_tailscale_device_name(container).await,
        };

        let user = container
            .container_user
            .clone()
            .unwrap_or("root".to_string());
        let cmd = "echo 'SSH connection test'";

        info!(
            "[Runpod Controller] Testing SSH connectivity to {} as user {}",
            hostname.clone(),
            user
        );

        let hostname_for_ssh = hostname.clone();
        // Set a short timeout for the SSH check (5 seconds)
        match tokio::time::timeout(
            std::time::Duration::from_secs(5),
            tokio::task::spawn_blocking(move || {
                run_ssh_command_ts(
                    &hostname_for_ssh,
                    cmd.split_whitespace().map(|s| s.to_string()).collect(),
                    false,
                    false,
                    Some(&user),
                )
            }),
        )
        .await
        {
            Ok(Ok(_)) => {
                info!(
                    "[Runpod Controller] SSH connection successful to {}",
                    hostname
                );
                Ok(true)
            }
            Ok(Err(e)) => {
                info!(
                    "[Runpod Controller] SSH connection failed to {}: {}",
                    hostname, e
                );
                Ok(false)
            }
            Err(_) => {
                info!(
                    "[Runpod Controller] SSH connection timed out to {}",
                    hostname
                );
                Ok(false)
            }
        }
    }

    // A helper for substituting both $VAR and ${VAR} with values from env_map
    fn expand_variables(&self, input: &str, env_map: &HashMap<String, String>) -> String {
        let re = Regex::new(r"\$([A-Za-z0-9_]+)|\$\{([A-Za-z0-9_]+)\}").unwrap();
        re.replace_all(input, |caps: &regex::Captures| {
            // The capture groups are 1 and 2 respectively for $VAR or ${VAR}
            if let Some(key) = caps.get(1) {
                // If the user wrote something like $VAR
                env_map.get(key.as_str()).cloned().unwrap_or_default()
            } else if let Some(key) = caps.get(2) {
                // If the user wrote something like ${VAR}
                env_map.get(key.as_str()).cloned().unwrap_or_default()
            } else {
                String::new()
            }
        })
        .to_string()
    }

    async fn determine_volumes_config(
        &self,
        name: &str,
        namespace: &str,
        model: Vec<V1VolumePath>,
        env_map: &HashMap<String, String>,
        db: &DatabaseConnection,
        owner: &str,
    ) -> anyhow::Result<VolumeConfig> {
        let mut volume_paths = Vec::new();
        let mut symlinks = Vec::new();
        let cache_dir = "/nebu/cache".to_string();

        for path in model {
            // Expand environment variables in source/dest prior to rewriting
            let expanded_source = self.expand_variables(&path.source, env_map);
            let expanded_dest = self.expand_variables(&path.dest, env_map);

            debug!("[Runpod Controller] Expanded source: {}", expanded_source);
            debug!("[Runpod Controller] Expanded dest: {}", expanded_dest);

            // Check if destination path is local (not starting with s3:// or other remote protocol)
            let is_local_destination = !expanded_dest.starts_with("s3://")
                && !expanded_dest.starts_with("gs://")
                && !expanded_dest.starts_with("azure://")
                && !expanded_dest.starts_with("nebu://");

            let final_dest = if expanded_dest.starts_with("nebu://") {
                // Extract the volume name and remaining path from nebu://{volume}/rest/of/path
                let path_without_prefix = expanded_dest.strip_prefix("nebu://").unwrap_or("");
                let mut parts = path_without_prefix.splitn(2, '/');

                let volume_name = match parts.next() {
                    Some(volume_name) => volume_name,
                    None => {
                        error!(
                            "[Runpod Controller] Failed to parse nebu:// path: {}",
                            expanded_dest
                        );
                        return Err(anyhow::anyhow!(
                            "[Runpod Controller] Failed to parse nebu:// path: {}",
                            expanded_dest
                        ));
                    }
                };
                debug!("[Runpod Controller] Volume name: {}", volume_name);
                let remaining_path = parts.next().unwrap_or("");

                debug!(
                    "[Runpod Controller] Parsed nebu:// path - volume: {}, remaining: {} with owners: {} and namespace: {}",
                    volume_name, remaining_path, owner, namespace
                );

                // Query for the volume
                match Query::find_volume_by_namespace_name_and_owners(
                    db,
                    namespace,
                    volume_name,
                    &[owner], // Use the container's owner for authorization
                )
                .await
                {
                    Ok(volume) => {
                        // Combine the volume's source with the remaining path
                        let final_path = if remaining_path.is_empty() {
                            volume.source
                        } else {
                            format!(
                                "{}/{}",
                                volume.source.trim_end_matches('/'),
                                remaining_path.trim_start_matches('/')
                            )
                        };

                        debug!(
                            "[Runpod Controller] Resolved nebu:// path to: {}",
                            final_path
                        );
                        final_path
                    }
                    Err(e) => {
                        error!(
                            "[Runpod Controller] Failed to find volume '{}' in namespace '{}' with owners '{}': {}",
                            volume_name, namespace, owner, e
                        );
                        return Err(anyhow::anyhow!(
                            "[Runpod Controller] Failed to find volume '{}' in namespace '{}': {}",
                            volume_name,
                            namespace,
                            e
                        ));
                    }
                }
            } else if is_local_destination {
                // For local paths, we'll sync to cache directory instead
                let path_without_leading_slash = expanded_dest.trim_start_matches('/');
                let cache_path = format!("{}/{}", cache_dir, path_without_leading_slash);

                // Add a symlink from the cache path to the original destination path
                symlinks.push(SymlinkConfig {
                    source: cache_path.clone(),
                    symlink_path: expanded_dest.clone(),
                });

                cache_path
            } else {
                expanded_dest
            };

            let volume_path = VolumePath {
                source: expanded_source,
                dest: final_dest,
                resync: path.resync,
                continuous: path.continuous,
                driver: path.driver,
            };
            volume_paths.push(volume_path);
        }

        let volume_config = VolumeConfig {
            paths: volume_paths,
            cache_dir,
            symlinks,
        };
        debug!("[Runpod Controller] Volume config: {:?}", volume_config);
        Ok(volume_config)
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
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await?;

        info!("[Runpod Controller] Using name: {}", model.name);
        let gpu_types_response = match self.runpod_client.list_gpu_types_graphql().await {
            Ok(response) => response,
            Err(e) => {
                error!("[Runpod Controller] Error fetching GPU types: {:?}", e);

                // More detailed error information
                if let Some(status) = e.status() {
                    error!("[Runpod Controller] HTTP Status: {}", status);
                }

                return Err(Box::<dyn std::error::Error + Send + Sync>::from(format!(
                    "Error fetching GPU types: {:?}",
                    e
                )));
            }
        };

        let mut runpod_gpu_type_id: String = "NVIDIA_TESLA_T4".to_string(); // Default value
        let mut nebu_gpu_type_id: String = "NVIDIA_TESLA_T4".to_string(); // Default value
        let mut requested_gpu_count = 1; // Default value
        let mut _datacenter_id = String::from("US"); // Default value
        let mut available_gpu_types = Vec::new();

        // Extract available GPU types from the response
        if let Some(data) = &gpu_types_response.data {
            // Log the available GPU types
            for gpu_type in data {
                available_gpu_types.push(gpu_type.id.clone());

                // Log GPU type details
                let memory_str = match gpu_type.memory_in_gb {
                    Some(mem) => format!("{} GB", mem),
                    None => "Unknown".to_string(),
                };

                info!(
                    "[Runpod Controller] GPU Type: {}, Display Name: {}, Memory: {}",
                    gpu_type.id, gpu_type.display_name, memory_str
                );
            }
            info!(
                "[Runpod Controller] Available GPU types: {:?}",
                available_gpu_types
            );
        }
        info!(
            "[Runpod Controller] GPU types response: {:?}",
            gpu_types_response
        );

        // Parse accelerators if provided
        if let Some(accelerators) = &model.accelerators {
            if !accelerators.is_empty() {
                let mut found_valid_accelerator = false;

                // Try each accelerator in the list until we find one that works
                for accelerator in accelerators {
                    // Parse the accelerator (format: "count:type")
                    info!("[Runpod Controller] Accelerator: {}", accelerator);
                    let parts: Vec<&str> = accelerator.split(':').collect();
                    if parts.len() == 2 {
                        if let Ok(count) = parts[0].parse::<i32>() {
                            // Convert from our accelerator name to RunPod's GPU type ID
                            if let Some(runpod_gpu_name) = self.accelerator_map().get(parts[1]) {
                                info!("[Runpod Controller] RunPod GPU name: {}", runpod_gpu_name);
                                // Check if this GPU type is available on RunPod
                                if available_gpu_types.is_empty()
                                    || available_gpu_types.contains(runpod_gpu_name)
                                {
                                    // This accelerator is available, use it
                                    requested_gpu_count = count;
                                    runpod_gpu_type_id = runpod_gpu_name.clone();
                                    nebu_gpu_type_id = parts[1].to_string();
                                    found_valid_accelerator = true;

                                    info!(
                                        "[Runpod Controller] Using accelerator: {} (count: {})",
                                        runpod_gpu_type_id, requested_gpu_count
                                    );

                                    // We found a valid accelerator, stop looking
                                    break;
                                } else {
                                    info!(
                                        "[Runpod Controller] Accelerator type '{}' is not available, trying next option",
                                        runpod_gpu_name
                                    );
                                }
                            } else {
                                info!(
                                    "[Runpod Controller] Unknown accelerator type: {}, trying next option",
                                    parts[1]
                                );
                            }
                        }
                    }
                }

                // If we couldn't find any valid accelerator, return an error
                if !found_valid_accelerator {
                    error!(
                        "[Runpod Controller] None of the requested accelerator types are available. Available types: {:?}",
                        available_gpu_types
                    );
                    return Err(Box::<dyn std::error::Error + Send + Sync>::from(
                        "None of the requested accelerator types are available on RunPod"
                            .to_string(),
                    ));
                }
            }
        }

        // --- ADDED: Generate SSH key pair and store the private key as a secret ---
        let (ssh_private_key, ssh_public_key) = keys::generate_ssh_keypair()?;
        debug!(
            "[Runpod Controller] Generated SSH private key: {}",
            ssh_private_key
        );
        debug!(
            "[Runpod Controller] Generated SSH public key: {}",
            ssh_public_key
        );
        mutation::Mutation::store_ssh_keypair(
            db,
            &model.id,
            &model.namespace,
            &ssh_private_key,
            &ssh_public_key,
            &model.owner,
            None,
        )
        .await?;

        let mut env_vec = Vec::new();

        match std::env::var("RUNPOD_PUBLIC_KEY") {
            Ok(runpod_public_key) => {
                info!(
                    "[Runpod Controller] Using static RUNPOD_PUBLIC_KEY environment variable: {:?}",
                    runpod_public_key
                );
                // env_vec.push(runpod::EnvVar {
                //     key: "RUNPOD_SSH_PUBLIC_KEY".to_string(),
                //     value: runpod_public_key,
                // });
            }
            Err(_) => {
                info!(
                    "[Runpod Controller] Using generated RUNPOD_PUBLIC_KEY: {}",
                    ssh_public_key
                );
                env_vec.push(runpod::EnvVar {
                    key: "RUNPOD_SSH_PUBLIC_KEY".to_string(),
                    value: ssh_public_key,
                });
            }
        }

        let common_env = self.get_common_env(&model, db).await;
        for (key, value) in common_env.clone() {
            env_vec.push(runpod::EnvVar { key, value });
        }

        debug!(
            "[Runpod Controller] Getting container default user for image: {}",
            model.image
        );
        // A "match" to catch errors, log them, and possibly do something else
        let (_parsed_manifest, container_user) = match pull_and_parse_config(&model.image).await {
            Ok((parsed_manifest, container_user)) => (parsed_manifest, container_user),
            Err(err) => {
                error!(
                    "[Runpod Controller] Failed pulling/parsing config for image '{}': {:#}",
                    model.image, err
                );
                // We can either choose to return immediately with Err or
                // fallback to defaults. For now, just return the error:
                return Err(err);
            }
        };

        debug!(
            "[Runpod Controller] Container default user from OCI config: {}",
            container_user
        );

        // If you want a graceful fallback to 'root' in case container_user is empty:
        let final_user = if container_user.is_empty() {
            "root".to_string()
        } else {
            container_user.clone()
        };
        Mutation::update_container_user(db, model.id.clone(), Some(final_user)).await?;

        match model.parse_env() {
            Ok(Some(env)) => {
                // We have a valid, non-empty list of environment variables.
                for env_var in env {
                    let value = match env_var.secret_name {
                        Some(secret_name) => {
                            let secret_model =
                                match crate::query::Query::find_secret_by_namespace_and_name(
                                    db,
                                    &model.namespace,
                                    &secret_name,
                                )
                                .await?
                                {
                                    Some(secret) => secret,
                                    None => {
                                        error!(
                                            "[Runpod Controller] Secret not found: {}",
                                            secret_name
                                        );
                                        continue;
                                    }
                                };
                            secret_model.decrypt_value().ok()
                        }
                        None => env_var.value.clone(),
                    };

                    if value.is_none() {
                        error!(
                            "[Runpod Controller] Failed to find value for key: {}",
                            env_var.key
                        );
                        continue;
                    }
                    env_vec.push(runpod::EnvVar {
                        key: env_var.key.clone(),
                        value: value.unwrap(),
                    });
                }
                info!("[Runpod Controller] Successfully parsed and added environment variables from model");
            }
            Ok(None) => {
                // parse_env() returned Ok, but the database column was None or empty.
                info!("[Runpod Controller] No environment variables configured in database");
            }
            Err(e) => {
                // A serialization error occurred while parsing
                error!(
                    "[Runpod Controller] Failed to parse environment variables: {}",
                    e
                );
                // Decide how you want to handle the error (return early, ignore, etc.)
            }
        }
        info!("[Runpod Controller] Environment variables: {:?}", env_vec);

        let env_map: HashMap<String, String> = env_vec
            .iter()
            .map(|env| (env.key.clone(), env.value.clone()))
            .collect();

        // Add NEBU_SYNC_CONFIG environment variable with serialized volumes configuration
        match model.parse_volumes() {
            Ok(Some(volumes)) => {
                // We got a valid Vec of V1VolumePath. Proceed as before.
                debug!("[Runpod Controller] Parsing volumes: {:?}", volumes);
                debug!("[Runpod Controller] Environment map: {:?}", env_map);
                debug!("[Runpod Controller] Namespace: {}", model.namespace);
                debug!("[Runpod Controller] Name: {}", model.name);
                debug!("[Runpod Controller] Owner: {}", model.owner);
                let volume_config = match self
                    .determine_volumes_config(
                        &model.name,
                        &model.namespace,
                        volumes,
                        &env_map, // Now passing the HashMap instead of Vec<EnvVar>
                        db,
                        &model.owner,
                    )
                    .await
                {
                    Ok(volume_config) => volume_config,
                    Err(e) => {
                        error!(
                            "[Runpod Controller] Failed to determine volumes config: {}",
                            e
                        );
                        return Err(e.into());
                    }
                };
                info!("[Runpod Controller] Volume config: {:?}", volume_config);

                match serde_yaml::to_string(&volume_config) {
                    Ok(serialized_volumes) => {
                        env_vec.push(runpod::EnvVar {
                            key: "NEBU_SYNC_CONFIG".to_string(),
                            value: serialized_volumes,
                        });
                        info!("[Runpod Controller] Added NEBU_SYNC_CONFIG environment variable");
                    }
                    Err(e) => {
                        error!(
                            "[Runpod Controller] Failed to serialize volumes configuration: {}",
                            e
                        );
                        // Continue without this env var rather than failing the whole operation
                    }
                }
            }
            Ok(None) => {
                // parse_volumes() returned Ok, but no volumes were present
                info!("[Runpod Controller] No volumes configured, skipping NEBU_SYNC_CONFIG");
            }
            Err(e) => {
                // A serialization error occurred, so handle it (log, return, etc.)
                error!("[Runpod Controller] Failed to parse volumes: {}", e);
                // If you'd prefer to ignore the parse failure and still run, you could do so here
            }
        }
        info!("[Runpod Controller] Environment variables: {:?}", env_vec);

        let hostname = self.get_tailscale_device_name(&model).await;
        info!("[Runpod Controller] Hostname: {}", hostname);

        let docker_command = self.build_command(&model, &hostname);
        info!("[Runpod Controller] Docker command: {:?}", docker_command);

        let datacenter_id =
            if model.accelerators.is_some() && !model.accelerators.as_ref().unwrap().is_empty() {
                // Only find datacenters with desired GPU if accelerators are requested
                let datacenters = self
                    .runpod_client
                    .find_datacenters_with_desired_gpu(&runpod_gpu_type_id, requested_gpu_count)
                    .await
                    .map_err(|e| format!("Failed to find datacenters: {}", e))?;

                // grab the first datacenter's ID
                datacenters
                    .first()
                    .ok_or("No datacenters available for the requested GPU type")?
                    .id
                    .clone()
            } else {
                // For CPU-only workloads, default to EU-RO-1
                "EU-RO-1".to_string()
            };

        let volume = match self
            .runpod_client
            .ensure_volume_in_datacenter(
                &model
                    .owner
                    .replace(".", "-")
                    .replace("@", "-")
                    .replace("+", "-")
                    .replace("_", "-"),
                &datacenter_id,
                500,
            )
            .await
        {
            Ok(vol) => vol,
            Err(e) => {
                panic!("Error ensuring volume in datacenter: {:?}", e);
            }
        };

        // 5) Create an on-demand instance instead of a spot instance
        let create_request =
            if model.accelerators.is_some() && !model.accelerators.as_ref().unwrap().is_empty() {
                // GPU workload
                CreateOnDemandPodRequest {
                    cloud_type: Some("SECURE".to_string()),
                    gpu_count: Some(requested_gpu_count),
                    volume_in_gb: Some(500),
                    compute_type: Some("GPU".to_string()),
                    container_disk_in_gb: Some(1000),
                    min_vcpu_count: Some(8),
                    min_memory_in_gb: Some(30),
                    gpu_type_id: Some(runpod_gpu_type_id),
                    name: Some(model.id.clone()),
                    image_name: Some(model.image.clone()),
                    docker_args: None,
                    docker_entrypoint: docker_command.clone(),
                    ports: Some(vec!["22/tcp".to_string(), "8080/http".to_string()]),
                    env: env_vec,
                    network_volume_id: Some(volume.id),
                    volume_mount_path: Some("/nebu/cache".to_string()),
                }
            } else {
                // CPU-only workload
                CreateOnDemandPodRequest {
                    cloud_type: Some("SECURE".to_string()),
                    gpu_count: None, // No GPUs for CPU workload
                    volume_in_gb: Some(500),
                    compute_type: Some("CPU".to_string()),
                    container_disk_in_gb: Some(1000),
                    min_vcpu_count: Some(8),
                    min_memory_in_gb: Some(30),
                    gpu_type_id: None, // No GPU type for CPU workload
                    name: Some(model.id.clone()),
                    image_name: Some(model.image.clone()),
                    docker_args: None,
                    docker_entrypoint: docker_command.clone(),
                    ports: Some(vec!["22/tcp".to_string(), "8080/http".to_string()]),
                    env: env_vec,
                    network_volume_id: Some(volume.id),
                    volume_mount_path: Some("/nebu/cache".to_string()),
                }
            };

        info!(
            "[Runpod Controller] Creating on-demand pod with request: {:?}",
            create_request
        );

        // Attempt to create the on-demand pod - directly await
        let pod_id = match self
            .runpod_client
            .create_on_demand_pod(create_request)
            .await
        {
            Ok(resp) => {
                if let Some(pod) = resp.data {
                    info!(
                        "[Runpod Controller] Successfully created On-Demand Pod '{}' (id = {}) on RunPod!",
                        model.id, pod.id
                    );

                    Mutation::update_container_resource_name(db, model.id.clone(), pod.id.clone())
                        .await?;

                    info!(
                        "[Runpod Controller] Updating container status to Created, and accelerator to {}",
                        nebu_gpu_type_id
                    );
                    Mutation::update_container_status(
                        db,
                        model.id.clone(),
                        Some(ContainerStatus::Created.to_string()),
                        None,
                        Some(nebu_gpu_type_id),
                        None,
                        Some(format!("http://{}", hostname)),
                        None,
                        None,
                    )
                    .await?;

                    Mutation::update_container_resource_cost_per_hr(
                        db,
                        model.id.clone(),
                        pod.cost_per_hr,
                    )
                    .await?;
                    pod.id
                } else {
                    return Err(format!(
                        "On-Demand Pod creation returned empty data for job '{}'",
                        model.id
                    )
                    .into());
                }
            }
            Err(e) => {
                return Err(format!(
                    "Error creating on-demand pod on RunPod for '{}': {:?}",
                    model.id, e
                )
                .into());
            }
        };

        info!(
            "[Runpod Controller] Job {} created on RunPod with pod ID {}",
            model.id, pod_id
        );

        Ok(pod_id)
    }

    fn build_command(&self, model: &containers::Model, hostname: &str) -> Option<Vec<String>> {
        let cmd = model.command.clone()?;

        let _proxy_value = "socks5h://127.0.0.1:1055".to_string();

        // Statements to install curl if missing:
        let curl_install = r#"
            echo "[DEBUG] Installing curl (if not present)..."
            if ! command -v curl &> /dev/null; then
                apt-get update && apt-get install -y curl \
                || (apk update && apk add --no-cache curl) \
                || echo 'Failed to install curl'
            fi
        "#;

        // Statements to install nebu if missing:
        let nebu_install = r#"
            echo "[DEBUG] Installing nebu (if not present)..."
            if ! command -v nebu &> /dev/null; then
                curl -s https://raw.githubusercontent.com/agentsea/nebulous/main/remote_install.sh | bash \
                || echo 'Failed to install nebu'
            fi
        "#;

        // Here's the new portion for checking tailscale before installing
        let tailscale_install = r#"
    echo "[DEBUG] Installing tailscale (if not present)..."
    if ! command -v tailscale &> /dev/null; then
        echo "[DEBUG] Tailscale not installed. Installing..."
        curl -fsSL https://tailscale.com/install.sh | sh
    else
        echo "[DEBUG] Tailscale already installed."
    fi
"#;

        let log_file = "$HOME/.logs/nebu_container.log";

        // export ALL_PROXY={proxy_value}  # TODO: this is problematic for DNS resolution but we may need it
        // export HTTP_PROXY={proxy_value}
        // export HTTPS_PROXY={proxy_value}

        // Wrap the user command in parentheses and add a semicolon to ensure it's treated as a complete statement
        let base_script = format!(
            r#"
    mkdir -p "$HOME/.logs"
    set -x
    exec > >(tee -a {log_file}) 2>&1

    echo "[DEBUG] Starting setup..."
    {curl_install}
    echo "[DEBUG] Done installing curl..."
    
    {nebu_install}
    echo "[DEBUG] Done installing nebu; checking version..."
    nebu --version

    echo "[DEBUG] Setting HF_HOME to /nebu/cache/huggingface"
    mkdir -p /nebu/cache/huggingface

    {tailscale_install}

    echo "[DEBUG] Starting tailscale daemon ..."
    tailscaled --tun=userspace-networking --socks5-server=localhost:1055 --outbound-http-proxy-listen=localhost:1055  > "$HOME/.logs/tailscaled.log" 2>&1 &

    echo "[DEBUG] Waiting for tailscale to start..."
    for i in $(seq 1 10); do
        if tailscale status >/dev/null 2>&1; then
            echo "[DEBUG] Tailscale is up"
            break
        else
            echo "[DEBUG] Tailscale not up yet, retrying..."
            sleep 1
        fi
    done

    echo "[DEBUG] Starting tailscale up..."
    tailscale up --auth-key=$TS_AUTHKEY --hostname="{hostname}" --ssh

    echo "[DEBUG] Invoking nebu sync..."
    nebu sync volumes --config /nebu/sync.yaml --interval-seconds 5 \
        --create-if-missing --config-from-env

    echo "[DEBUG] Invoking nebu sync background..."
    nebu sync volumes --config /nebu/sync.yaml --interval-seconds 5 \
        --create-if-missing --watch --background --block-once --config-from-env
    
    echo "[DEBUG] All done with base_command; now your user command: {cmd}"
    ({cmd}) # Wrap in parentheses and add semicolon
    "#,
            curl_install = curl_install,
            nebu_install = nebu_install,
            cmd = cmd
        );

        // 2) Always wait for final sync - now without the leading &&
        let wait_script = r#"
echo "[DEBUG] Waiting for final sync..."
nebu sync wait --config /nebu/sync.yaml --interval-seconds 5
"#;

        // 3) Only if restart == Never, mark done and loop forever - now without the leading &&
        let never_script = if model.restart == RestartPolicy::Never.to_string() {
            r#"
echo "[DEBUG] Writing /done.txt..."
echo "done" > /done.txt
while true; do
    echo ">>>all done"
    sleep 3
done
"#
        } else {
            ""
        };

        // 4) Combine them into our final script
        let final_script = format!(
            r#"{base_script}
{wait_script}
{never_script}
"#,
            base_script = base_script,
            wait_script = wait_script,
            never_script = never_script
        );

        info!("[Runpod Controller] Final script: {}", final_script);

        Some(vec!["bash".to_string(), "-c".to_string(), final_script])
    }

    /// Checks if `/done.txt` exists in the container. Returns `Ok(true)` if found, `Ok(false)` otherwise.
    pub async fn check_done_file(
        &self,
        container_id: &str,
        container_user: &str,
        db: &DatabaseConnection,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        // 1) Fetch the container from the database
        let container_model =
            match crate::query::Query::find_container_by_id(db, container_id.to_string()).await? {
                Some(model) => model,
                None => return Err(format!("Container {} not found", container_id).into()),
            };

        // 2) Retrieve the RunPod Pod ID (stored in container.resource_name)
        let resource_name = container_model
            .clone()
            .resource_name
            .ok_or_else(|| format!("No resource_name found for container {}", container_id))?;

        info!("[Runpod Controller] Resource name: {:?}", resource_name);
        info!("[Runpod Controller] Fetching pod host id...");

        let pod_host_id = match self.runpod_client.get_pod_host_id(&resource_name).await? {
            Some(pod_host_id) => pod_host_id,
            None => {
                return Err(format!("No pod host id found for container {}", container_id).into())
            }
        };
        info!("[Runpod Controller] Fetched pod host id: {:?}", pod_host_id);

        // 3) Fetch the SSH key pair from the database
        // let (maybe_private_key, maybe_public_key) =
        //     crate::query::Query::get_ssh_keypair(db, &container_model.id).await?;

        // let ssh_private_key = maybe_private_key
        //     .ok_or_else(|| format!("No SSH private key found for container {}", container_id))?;
        // let _ssh_public_key = maybe_public_key
        //     .ok_or_else(|| format!("No SSH public key found for container {}", container_id))?;

        // debug!("[Runpod Controller] SSH private key: {}", ssh_private_key);
        // debug!("[Runpod Controller] SSH public key: {}", _ssh_public_key);

        // 4) Form a command that checks existence of /done.txt
        //    - On success, it prints '1', otherwise '0'
        let cmd = "test -f /done.txt && echo 1 || echo 0";
        info!("[Runpod Controller] Done file check command: {}", cmd);

        let hostname = match container_model.tailnet_ip {
            Some(ip) => ip,
            None => self.get_tailscale_device_name(&container_model).await,
        };

        info!("[Runpod Controller] Current hostname for ssh: {}", hostname);
        // 5) Execute the command over SSH
        let output = match run_ssh_command_ts(
            &hostname,
            cmd.split_whitespace().map(|s| s.to_string()).collect(),
            false,
            false,
            Some(container_user),
        ) {
            Ok(output) => output,
            Err(e) => return Err(e.into()),
        };
        info!(
            "[Runpod Controller] Check done file output: '{}'",
            output.trim()
        );

        info!("raw output.trim() = {:?}", output.trim());
        info!("len = {}", output.trim().len());
        for (i, b) in output.trim().as_bytes().iter().enumerate() {
            info!("byte[{}] = {:#04x}", i, b);
        }

        if output.trim() == "1".to_string() {
            info!("[Runpod Controller] Done file is present!");
        } else {
            info!("[Runpod Controller] Done file is not present");
        }

        // 6) If output contains "1", the file exists, otherwise it doesn't
        let file_exists = output.trim() == "1";
        info!("[Runpod Controller] File exists: {}", file_exists);
        Ok(file_exists)
    }

    // Add this new function to the RunpodPlatform impl block
    async fn perform_health_check(
        &self,
        container: &containers::Model,
        health_check: &V1ContainerHealthCheck,
        current_status: &ContainerStatus,
        db: &DatabaseConnection,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Get the container's tailnet IP or hostname
        let hostname = match &container.tailnet_ip {
            Some(ip) => ip.clone(),
            None => self.get_tailscale_device_name(container).await,
        };

        // Build the health check URL
        let port = health_check.port.unwrap_or(8080);
        let path = health_check.path.as_ref().map_or("/health", |s| s);
        let protocol = health_check.protocol.as_ref().map_or("http", |s| s);
        let url = format!("{}://{}:{}{}", protocol, hostname, port, path);

        info!("[Runpod Controller] Checking health at URL: {}", url);

        // Create a client with appropriate timeout
        let timeout_duration = match &health_check.timeout {
            Some(timeout_str) => std::time::Duration::from_secs(
                humantime::parse_duration(timeout_str)
                    .unwrap_or(std::time::Duration::from_secs(5))
                    .as_secs(),
            ),
            None => std::time::Duration::from_secs(5),
        };

        let client = reqwest::Client::builder()
            .timeout(timeout_duration)
            .build()
            .unwrap_or_default();

        // Perform the health check
        match client.get(&url).send().await {
            Ok(response) => {
                if response.status().is_success() {
                    info!(
                        "[Runpod Controller] Health check passed for {}",
                        container.id
                    );
                    if *current_status != ContainerStatus::Running {
                        Mutation::update_container_status(
                            db,
                            container.id.clone(),
                            None,
                            None,
                            None,
                            None,
                            None,
                            None,
                            Some(true),
                        )
                        .await?;
                    }
                } else {
                    warn!(
                        "[Runpod Controller] Health check failed for {} with status: {}",
                        container.id,
                        response.status()
                    );
                    if *current_status == ContainerStatus::Running {
                        Mutation::update_container_status(
                            db,
                            container.id.clone(),
                            None,
                            None,
                            None,
                            None,
                            None,
                            None,
                            Some(false),
                        )
                        .await?;
                    }
                }
            }
            Err(e) => {
                warn!(
                    "[Runpod Controller] Health check request failed for {}: {}",
                    container.id, e
                );
                if *current_status == ContainerStatus::Running {
                    Mutation::update_container_status(
                        db,
                        container.id.clone(),
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        Some(false),
                    )
                    .await?;
                }
            }
        }

        Ok(())
    }
}

impl ContainerPlatform for RunpodPlatform {
    /// Asynchronously run a container by provisioning a RunPod spot or on-demand pod.
    async fn declare(
        &self,
        config: &V1ContainerRequest,
        db: &DatabaseConnection,
        user_profile: &V1UserProfile,
        owner_id: &str,
        namespace: &str,
    ) -> Result<V1Container, Box<dyn std::error::Error + Send + Sync>> {
        let name = config
            .metadata
            .as_ref()
            .and_then(|meta| Some(meta.name.clone()))
            .unwrap_or_else(|| {
                // Generate a random human-friendly name using petname
                petname::petname(3, "-")
            });
        info!("[Runpod Controller] Using name: {:?}", name);
        let gpu_types_response = match self.runpod_client.list_gpu_types_graphql().await {
            Ok(response) => response,
            Err(e) => {
                error!("[Runpod Controller] Error fetching GPU types: {:?}", e);

                // More detailed error information
                if let Some(status) = e.status() {
                    error!("[Runpod Controller] HTTP Status: {}", status);
                }

                return Err(Box::<dyn std::error::Error + Send + Sync>::from(format!(
                    "Error fetching GPU types: {:?}",
                    e
                )));
            }
        };

        let mut runpod_gpu_type_id: String = "NVIDIA_TESLA_T4".to_string(); // Default value
        let mut requested_gpu_count = 1; // Default value
        let mut datacenter_id = String::from("US"); // Default value
        let mut available_gpu_types = Vec::new();
        let mut resource_cost_per_hr: Option<f64> = None;

        // Extract available GPU types from the response
        if let Some(data) = &gpu_types_response.data {
            // Log the available GPU types
            for gpu_type in data {
                available_gpu_types.push(gpu_type.id.clone());

                // Log GPU type details
                let memory_str = match gpu_type.memory_in_gb {
                    Some(mem) => format!("{} GB", mem),
                    None => "Unknown".to_string(),
                };

                info!(
                    "[Runpod Controller] GPU Type: {}, Display Name: {}, Memory: {}",
                    gpu_type.id, gpu_type.display_name, memory_str
                );
            }
            info!(
                "[Runpod Controller] Available GPU types: {:?}",
                available_gpu_types
            );
        }
        info!(
            "[Runpod Controller] GPU types response: {:?}",
            gpu_types_response
        );

        // Parse accelerators if provided
        if let Some(accelerators) = &config.accelerators {
            if !accelerators.is_empty() {
                let mut found_valid_accelerator = false;

                // Try each accelerator in the list until we find one that works
                for accelerator in accelerators {
                    // Parse the accelerator (format: "count:type")
                    info!("[Runpod Controller] Accelerator: {}", accelerator);
                    let parts: Vec<&str> = accelerator.split(':').collect();
                    if parts.len() == 2 {
                        if let Ok(count) = parts[0].parse::<i32>() {
                            // Convert from our accelerator name to RunPod's GPU type ID
                            if let Some(runpod_gpu_name) = self.accelerator_map().get(parts[1]) {
                                info!("[Runpod Controller] RunPod GPU name: {}", runpod_gpu_name);
                                // Check if this GPU type is available on RunPod
                                if available_gpu_types.is_empty()
                                    || available_gpu_types.contains(runpod_gpu_name)
                                {
                                    // This accelerator is available, use it
                                    requested_gpu_count = count;
                                    runpod_gpu_type_id = runpod_gpu_name.clone();
                                    found_valid_accelerator = true;

                                    info!(
                                        "[Runpod Controller] Using accelerator: {} (count: {})",
                                        runpod_gpu_type_id, requested_gpu_count
                                    );

                                    // We found a valid accelerator, stop looking
                                    break;
                                } else {
                                    info!(
                                        "[Runpod Controller] Accelerator type '{}' is not available, trying next option",
                                        runpod_gpu_name
                                    );
                                }
                            } else {
                                info!(
                                    "[Runpod Controller] Unknown accelerator type: {}, trying next option",
                                    parts[1]
                                );
                            }
                        }
                    }
                }

                // If we couldn't find any valid accelerator, return an error
                if !found_valid_accelerator {
                    error!(
                        "[Runpod Controller] None of the requested accelerator types are available. Available types: {:?}",
                        available_gpu_types
                    );
                    return Err(Box::<dyn std::error::Error + Send + Sync>::from(
                        "None of the requested accelerator types are available on RunPod"
                            .to_string(),
                    ));
                }
            }
        }

        let id = ShortUuid::generate().to_string();
        info!("[Runpod Controller] ID: {}", id);

        debug!("[Runpod Controller] About to store agent key secret");
        // Restore user profile check (using the now-available full profile)
        debug!("[Runpod Controller] user_profile = {:?}", user_profile);
        if user_profile.token.is_none() {
            error!("[Runpod Controller] user_profile.token is None, cannot get agent key for container");
            return Err(Box::<dyn std::error::Error + Send + Sync>::from(
                "Cannot create container: user profile is missing authentication token".to_string(),
            ));
        }

        // Removed user_profile check as it's no longer passed.
        // The store_agent_key_secret function now needs to fetch the token itself if required.
        debug!(
            "[Runpod Controller] Storing agent key secret: id={}, owner_id={}",
            id, owner_id
        );
        match self
            .store_agent_key_secret(db, user_profile, &id, owner_id)
            .await
        {
            Ok(_) => debug!("[Runpod Controller] Successfully stored agent key secret"),
            Err(e) => {
                error!(
                    "[Runpod Controller] Failed to store agent key secret: {}",
                    e
                );
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

        // Fix the unwrap that's causing the panic
        let name = name.unwrap_or_else(|| {
            petname::petname(3, "-").unwrap_or_else(|| {
                // Fallback to a simple default name if petname fails
                format!("container-{}", ShortUuid::generate())
            })
        });

        debug!(
            "[Runpod Controller] Creating container record in database with GPU type ID: {}",
            runpod_gpu_type_id
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
                accelerator: Some(runpod_gpu_type_id.clone()),
                public_ports: None,
                cost_per_hr: None,
                tailnet_url: None,
                ready: None,
            }))),
            platform: Set(Some("runpod".to_string())),
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
                "[Runpod Controller] Failed to create container in database: {:?}",
                e
            );
            return Err(format!("Failed to create container in database: {:?}", e).into());
        } else {
            info!("[Runpod Controller] Created container {} in database ", id);
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
            platform: "runpod".to_string(),
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
                accelerator: Some(runpod_gpu_type_id.clone()),
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
            "[DEBUG:runpod.rs:reconcile] Entering reconcile for container {}",
            container.id
        );

        // If this container is assigned to a queue,
        // ensure no other container in that same queue is running/active.
        if let Some(queue_name) = &container.queue {
            // We check if the queue is free. We'll skip starting if not free.
            let queue_is_free =
                crate::query::Query::is_queue_free(db, queue_name, &container.id).await?;
            if !queue_is_free {
                // The queue is blocked by another container.
                // Set this container to "Queued" status if it's not already in a terminal state.
                info!(
                    "[Runpod Controller] Container {} is blocked by another container in queue '{}'; setting to Queued.",
                    container.id, queue_name
                );

                if let Ok(Some(parsed_status)) = container.parse_status() {
                    let current_status = parsed_status.status.unwrap_or_default();
                    // If it's not already queued (or in a terminal state), set to queued:
                    if current_status != ContainerStatus::Queued.to_string()
                        && !ContainerStatus::from_str(&current_status)
                            .unwrap_or(ContainerStatus::Invalid)
                            .is_inactive()
                    {
                        crate::mutation::Mutation::update_container_status(
                            db,
                            container.id.clone(),
                            Some(ContainerStatus::Queued.to_string()),
                            Some("Waiting in queue".to_string()),
                            None,
                            None,
                            None,
                            None,
                            None,
                        )
                        .await
                        .map_err(|e| format!("Failed to set container to Queued: {}", e))?;
                    }
                }

                return Ok(()); // do not proceed to create or watch
            } else {
                // Queue is free and this is the next container in line
                info!(
                    "[Runpod Controller] Container {} is next in queue '{}'; proceeding with start.",
                    container.id, queue_name
                );
            }
        }

        if let Ok(Some(parsed_status)) = container.parse_status() {
            let status_str = parsed_status
                .status
                .unwrap_or(ContainerStatus::Invalid.to_string());
            debug!(
                "[DEBUG:runpod.rs:reconcile] Container {} has status {}",
                container.id, status_str
            );

            let status = ContainerStatus::from_str(&status_str).unwrap_or(ContainerStatus::Invalid);

            if status.needs_start() {
                info!(
                    "[Runpod Controller] Container {} needs to be started",
                    container.id
                );
                if let Some(ds) = &container.desired_status {
                    if ds == &ContainerStatus::Running.to_string() {
                        info!("[Runpod Controller] Container {} has a desired status of 'running', creating...", container.id);
                        self.create(db, container.clone()).await?;
                    }
                } else {
                    info!("[Runpod Controller] Container {} does not have a desired status of 'running'", container.id);
                }
            }

            if status.needs_watch() {
                info!(
                    "[Runpod Controller] Container {} needs to be watched",
                    container.id
                );
                self.watch(db, container.clone()).await?;
            }
        }
        debug!(
            "[DEBUG:runpod.rs:reconcile] Completed reconcile for container {}",
            container.id
        );
        Ok(())
    }

    async fn exec(
        &self,
        container_id: &str,
        command: &str,
        db: &DatabaseConnection,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        // 1) Fetch the container from the database
        let container_model =
            match crate::query::Query::find_container_by_id(db, container_id.to_string()).await? {
                Some(model) => model,
                None => return Err(format!("Container {} not found", container_id).into()),
            };

        // // 2) Retrieve the RunPod Pod ID (stored in container.resource_name)
        // let resource_name = container_model
        //     .resource_name
        //     .ok_or_else(|| format!("No resource_name found for container {}", container_id))?;

        // let (maybe_private_key, maybe_public_key) =
        //     crate::query::Query::get_ssh_keypair(db, &container_model.id).await?;

        // // Now each is an Option<String>. You can handle them individually:
        // let ssh_private_key = maybe_private_key
        //     .ok_or_else(|| format!("No SSH private key found for container {}", container_id))?;
        // let _ssh_public_key = maybe_public_key
        //     .ok_or_else(|| format!("No SSH public key found for container {}", container_id))?;

        let hostname = match container_model.tailnet_ip {
            Some(ip) => ip,
            None => self.get_tailscale_device_name(&container_model).await,
        };

        // Then call exec_ssh_command or whatever you need:
        let output = match crate::ssh::exec::run_ssh_command_ts(
            &hostname,
            command.split_whitespace().map(|s| s.to_string()).collect(),
            false,
            false,
            Some(
                &container_model
                    .container_user
                    .clone()
                    .unwrap_or("root".to_string()),
            ),
        ) {
            Ok(output) => output,
            Err(e) => return Err(e.into()),
        };

        // For now, just log the result; adapt as needed
        tracing::info!("[Runpod Controller] SSH command output:\n{}", output);

        Ok(output)
    }

    async fn logs(
        &self,
        container_id: &str,
        db: &DatabaseConnection,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let log_file = "$HOME/.logs/nebu_container.log";

        // 1) Fetch the container from the database
        let container_model =
            match crate::query::Query::find_container_by_id(db, container_id.to_string()).await? {
                Some(model) => model,
                None => return Err(format!("Container {} not found", container_id).into()),
            };

        // 2) Retrieve the RunPod Pod ID (stored in container.resource_name)
        // let resource_name = container_model
        //     .resource_name
        //     .ok_or_else(|| format!("No resource_name found for container {}", container_id))?;

        // // 3) Fetch the SSH key pair from the database
        // let (maybe_private_key, maybe_public_key) =
        //     crate::query::Query::get_ssh_keypair(db, &container_model.id).await?;

        // let ssh_private_key = maybe_private_key
        //     .ok_or_else(|| format!("No SSH private key found for container {}", container_id))?;
        // let _ssh_public_key = maybe_public_key
        //     .ok_or_else(|| format!("No SSH public key found for container {}", container_id))?;

        // 4) SSH into the container and retrieve the log file
        //    Modify this as needed (for tailing, for instance).
        let command = format!("cat {}", log_file);

        let hostname = match container_model.tailnet_ip {
            Some(ip) => ip,
            None => self.get_tailscale_device_name(&container_model).await,
        };
        let output = match crate::ssh::exec::run_ssh_command_ts(
            &hostname,
            command.split_whitespace().map(|s| s.to_string()).collect(),
            false,
            false,
            Some(
                &container_model
                    .container_user
                    .clone()
                    .unwrap_or("root".to_string()),
            ),
        ) {
            Ok(output) => output,
            Err(e) => return Err(e.into()),
        };

        Ok(output)
    }

    async fn delete(
        &self,
        id: &str,
        db: &DatabaseConnection,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!(
            "[Runpod Controller] Attempting to delete container with name: {}",
            id
        );

        let container_model =
            match crate::query::Query::find_container_by_id(db, id.to_string()).await? {
                Some(model) => model,
                None => return Err(format!("Container {} not found", id).into()),
            };

        // First, list all pods to find the one with our name
        match self.runpod_client.list_pods().await {
            Ok(pods_response) => {
                if let Some(my_pods) = pods_response.data {
                    // Find the pod with matching name
                    let pod_to_delete = my_pods.pods.iter().find(|p| p.name == id);

                    if let Some(pod) = pod_to_delete {
                        info!(
                            "[Runpod Controller] Found pod with ID: {} for container: {}",
                            pod.id, id
                        );

                        // Stop the pod
                        match self.runpod_client.delete_pod(&pod.id).await {
                            Ok(_) => {
                                info!("[Runpod Controller] Successfully stopped pod: {}", pod.id);

                                // Update container status in database
                                if let Err(e) = crate::mutation::Mutation::update_container_status(
                                    &db,
                                    id.to_string(),
                                    Some(ContainerStatus::Stopped.to_string()),
                                    None,
                                    None,
                                    None,
                                    None,
                                    None,
                                    None,
                                )
                                .await
                                {
                                    error!("[Runpod Controller] Failed to update container status in database: {}", e);
                                    return Err(e.into());
                                } else {
                                    info!("[Runpod Controller] Updated container {} status to stopped", id);
                                    // Also remove the SSH key secrets
                                    let private_key_secret_id = format!("ssh-private-key-{}", id);
                                    let full_private_key_secret_id = format!(
                                        "{}/{}",
                                        container_model.namespace.clone(),
                                        private_key_secret_id
                                    );

                                    let public_key_secret_id = format!("ssh-public-key-{}", id);
                                    let full_public_key_secret_id = format!(
                                        "{}/{}",
                                        container_model.namespace.clone(),
                                        public_key_secret_id
                                    );
                                    match crate::mutation::Mutation::delete_secret_by_fullname(
                                        db,
                                        full_private_key_secret_id,
                                    )
                                    .await
                                    {
                                        Ok(delete_result) => {
                                            // Here, `delete_result` is the actual DeleteResult (e.g., rows_affected).
                                            info!("Deleted secret: {:?}", delete_result);
                                        }
                                        Err(err) => {
                                            // Handle or log the error
                                            error!("Failed to delete secret: {err}");
                                        }
                                    }
                                    match crate::mutation::Mutation::delete_secret_by_fullname(
                                        db,
                                        full_public_key_secret_id,
                                    )
                                    .await
                                    {
                                        Ok(delete_result) => {
                                            // Here, `delete_result` is the actual DeleteResult (e.g., rows_affected).
                                            info!("Deleted secret: {:?}", delete_result);
                                        }
                                        Err(err) => {
                                            // Handle or log the error
                                            error!("Failed to delete secret: {err}");
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                error!("[Runpod Controller] Failed to stop pod {}: {}", pod.id, e);
                                return Err(e.into());
                            }
                        }
                    } else {
                        info!("[Runpod Controller] No pod found with name: {}", id);
                    }
                } else {
                    error!("[Runpod Controller] No pods data returned from RunPod API");
                    return Err("No pods data returned from RunPod API".into());
                }
            }
            Err(e) => {
                error!("[Runpod Controller] Error listing pods: {}", e);
                return Err(e.into());
            }
        }

        Ok(())
    }

    fn accelerator_map(&self) -> HashMap<String, String> {
        let provider = RunPodProvider::new();
        provider.accelerator_map().clone()
    }
}

/// Returns true if the given error indicates a 404 Not Found response.
pub fn is_not_found(err: &reqwest::Error) -> bool {
    err.status() == Some(reqwest::StatusCode::NOT_FOUND)
}
