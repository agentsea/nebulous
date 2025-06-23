use crate::entities::containers;
use crate::query::Query;
use crate::state::AppState;
use std::sync::Arc;
use tokio::task::JoinHandle;
use tracing::{debug, error, info};

use anyhow::Result;
use dashmap::DashMap;
use once_cell::sync::Lazy;
use sea_orm::ActiveModelTrait;
use serde::{Deserialize, Serialize};
use short_uuid::ShortUuid;

/// A struct defining any reconciler metadata you want to store in `controller_data`.
/// This might hold more fields (timestamps, logs, etc.) if desired.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ReconcilerData {
    thread_id: Option<String>,
}

/// A global map from some container "thread_id" -> the running JoinHandle.
/// We'll store the `thread_id` in DB and look it up here to see if it's finished.
static CONTAINER_RECON_TASKS: Lazy<DashMap<String, JoinHandle<()>>> = Lazy::new(DashMap::new);

pub struct ContainerController {
    app_state: Arc<AppState>,
}

impl ContainerController {
    pub fn new(app_state: Arc<AppState>) -> Self {
        Self { app_state }
    }

    /// The main loop that spawns or skips reconciliation tasks (threads).
    /// Each container's `controller_data` field will hold the JSON specifying its `thread_id`.
    pub async fn reconcile(&self) {
        info!("[Container Controller] Starting container reconciliation process");

        match Query::find_all_active_containers(&self.app_state.db_pool).await {
            Ok(containers) => {
                debug!(
                    "[DEBUG:controller.rs:reconcile] Found {} containers to reconcile",
                    containers.len()
                );
                for container in containers {
                    debug!(
                        "[DEBUG:controller.rs:reconcile] Inspecting container {}",
                        container.id
                    );
                    // Attempt to parse `controller_data` as `ReconcilerData`.
                    let mut existing_data =
                        match container.parse_controller_data::<ReconcilerData>() {
                            Ok(Some(data)) => data,
                            _ => ReconcilerData { thread_id: None },
                        };

                    debug!(
                        "[DEBUG:controller.rs:reconcile] Existing thread_id = {:?}",
                        existing_data.thread_id,
                    );

                    // If there's already a thread_id, check if it's still alive.
                    if let Some(thread_id) = &existing_data.thread_id {
                        if let Some(handle_ref) = CONTAINER_RECON_TASKS.get(thread_id) {
                            // If handle still running, skip starting a new one.
                            debug!(
                                "[DEBUG:controller.rs:reconcile] handle_ref.is_finished() = {}",
                                handle_ref.is_finished()
                            );
                            if !handle_ref.is_finished() {
                                info!(
                                    "[Container Controller] Container {} has a running reconcile thread; skipping.",
                                    container.id
                                );
                                continue;
                            } else {
                                debug!(
                                    "[DEBUG:controller.rs] handle_ref.is_finished() = false; dropping ref",
                                );
                                // Drop the read reference to avoid deadlock
                                drop(handle_ref);

                                debug!(
                                    "[DEBUG:controller.rs] Removing finished thread_id = {} from map",
                                    thread_id
                                );

                                // Now remove from the map
                                let removed = CONTAINER_RECON_TASKS.remove(thread_id);
                                debug!("[DEBUG:controller.rs] remove(...) returned: {:?}", removed);
                            }
                        }
                    }

                    debug!(
                        "[DEBUG:controller.rs:reconcile] Spawning a new reconcile task for container {}",
                        container.id
                    );

                    // Otherwise, we spawn a fresh task.
                    let new_thread_id = ShortUuid::generate().to_string();
                    existing_data.thread_id = Some(new_thread_id.clone());

                    // Persist new `thread_id` in `controller_data`, so if we lose the process,
                    // we at least know which container was last assigned which thread ID.
                    if let Err(e) = Self::store_thread_id_in_db(
                        &container,
                        &existing_data,
                        &self.app_state.db_pool,
                    )
                    .await
                    {
                        error!(
                            "[Container Controller] Failed to store new thread_id for container {}: {:?}",
                            container.id, e
                        );
                        continue;
                    }

                    // Actually spawn a background task
                    let handle = tokio::spawn({
                        let db_pool = self.app_state.db_pool.clone();
                        let container_clone = container.clone();
                        async move {
                            info!(
                                "[Container Controller] Reconciling container {} in background task",
                                container_clone.id
                            );
                            debug!(
                                "[DEBUG:controller.rs:spawn] Calling platform.reconcile for container {}",
                                container_clone.id
                            );
                            // If your platform_factory is async, call it here.
                            let platform_name = container_clone
                                .platform
                                .clone()
                                .unwrap_or_else(|| "runpod".to_string());
                            let platform =
                                crate::resources::v1::containers::factory::platform_factory(
                                    platform_name,
                                )
                                .await;
                            let _ = platform.reconcile(&container_clone, &db_pool).await;
                            debug!(
                                "[DEBUG:controller.rs:spawn] Returned from platform.reconcile for container {}",
                                container_clone.id
                            );
                            info!(
                                "[Container Controller] Container {} reconcile task finished.",
                                container_clone.id
                            )
                        }
                    });

                    // Store handle in the map
                    CONTAINER_RECON_TASKS.insert(new_thread_id, handle);
                }
            }
            Err(e) => {
                error!(
                    "[Container Controller] Failed to fetch containers for reconciliation: {:?}",
                    e
                );
            }
        }
        debug!("[DEBUG:controller.rs:reconcile] Finished single reconcile pass");
    }

    /// Helper to save the updated `controller_data` back into the DB.
    async fn store_thread_id_in_db(
        container: &containers::Model,
        rec_data: &ReconcilerData,
        db_pool: &sea_orm::DatabaseConnection,
    ) -> Result<(), sea_orm::DbErr> {
        // Convert to JSON
        let data_json = serde_json::to_value(rec_data).unwrap_or_default();

        // Build an ActiveModel for the update
        let mut active = containers::ActiveModel::from(container.clone());
        active.controller_data = sea_orm::ActiveValue::Set(Some(data_json));

        // Perform the update
        active.update(db_pool).await?;
        Ok(())
    }
}

impl ContainerController {
    /// Spawns a background Tokio task to run the controller reconciliation loop
    pub fn spawn_reconciler(&self) -> tokio::task::JoinHandle<()> {
        let app_state_clone = Arc::clone(&self.app_state);

        tokio::spawn(async move {
            let controller = ContainerController::new(app_state_clone);

            // Create an infinite loop to continuously reconcile containers
            loop {
                controller.reconcile().await;
                // Add a delay between reconciliation cycles
                tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
            }
        })
    }
}
