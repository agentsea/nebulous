use crate::platforms::factory::create_platform_from_kind;
use crate::resources::v1::containers::models::V1Container;
use crate::resources::containers::ContainerModelVersion;
use crate::resources::v1::platforms::models::V1ContainerPlatform;
use crate::state::AppState;
use anyhow::{anyhow, Context};
use std::sync::Arc;
use tracing::{error, info, span, Instrument, Level};
use sea_orm::ActiveModelTrait;
use sea_orm::EntityTrait;
use crate::entities::containers::entity::Entity;


pub struct ContainerController<V>
where
    V: ContainerModelVersion,
{
    app_state: Arc<AppState>,
}

enum ContainerStatus {
    Pending,
    Scheduled,
    Creating,
    Running,
    Unreachable,
    Killed,
    Stopping,
    Finished,
    Failed,
}

impl ContainerStatus {
    fn from_string(status: &str) -> Self {
        match status {
            "Pending" => ContainerStatus::Pending,
            "Scheduled" => ContainerStatus::Scheduled,
            "Creating" => ContainerStatus::Creating,
            "Running" => ContainerStatus::Running,
            "Unreachable" => ContainerStatus::Unreachable,
            "Killed" => ContainerStatus::Killed,
            "Stopping" => ContainerStatus::Stopping,
            "Finished" => ContainerStatus::Finished,
            "Failed" => ContainerStatus::Failed,
            _ => panic!("Unknown container status"),
        }
    }
}

#[derive(Debug, Clone)]
struct ControllerContainer {
    id: String,
    status: ContainerStatus,
    platform: Option<String>,
}

trait ToControllerContainer {
    fn to_controller(&self) -> ControllerContainer;
}

impl ToControllerContainer for V1Container {
    fn to_controller(&self) -> ControllerContainer {
        ControllerContainer {
            id: self.metadata.id.clone(),
            platform: Some(self.platform.clone()),
            // TODO: Handle the status conversion (and make it a required field?)
            status: ContainerStatus::from_string(&self.status.clone().unwrap().status.unwrap()),
        }
    }
}

impl<V> ContainerController<V>
where
    V: ContainerModelVersion,
    V::Container: ToControllerContainer,
{
    pub fn new(app_state: Arc<AppState>) -> Self {
        Self { app_state }
    }

    pub async fn reconcile(&self) {
        let span = span!(Level::INFO, "ContainerController");

        async {
            info!("Starting reconciliation process");
            let paginator = entities::containers::Entity::find()
                .paginate(&self.app_state.db_pool, 100)
                .await?;
            let total_pages = paginator.num_pages().await?;

            for page in 1..=total_pages {
                let platforms = paginator.fetch_page(page).await?;
                for mut platform in platforms {
                    self.handle(platform).await;
                }
            }
            info!("Finished reconciliation process");
        }
        .instrument(span)
        .await;
    }

    pub async fn handle(&self, mut container: V::Container) -> anyhow::Result<()> {
        let container_status: ContainerStatus = ContainerStatus::Scheduled;

        match container_status {
            ContainerStatus::Pending | ContainerStatus::Scheduled => {
                self.schedule(container).await?;
            }
            ContainerStatus::Creating
            | ContainerStatus::Running
            | ContainerStatus::Unreachable
            | ContainerStatus::Stopping => {
                self.watch(container).await?;
            }
            ContainerStatus::Killed => {
                self.kill(container).await?;
            }
            ContainerStatus::Finished | ContainerStatus::Failed => {}
        }

        Ok(())
    }

    /// Schedules a container to run on a suitable platform.
    pub async fn schedule(&self, container: V::Container) -> anyhow::Result<V::Container> {
        let platform_spec = self.find_platform(container.clone()).await?;
        let platform: Box<dyn ContainerPlatform<V>> =
            create_platform_from_kind(platform_spec).await?;
        // TODO: Determine if the platform can take the container right now
        //       and adjust status accordingly

        // TODO: Run as separate task and return with "Creating"
        platform.create(container).await?
    }

    async fn find_platform(
        &self,
        container: V::Container,
    ) -> anyhow::Result<V::ContainerPlatform> {
        let container_spec = container.to_controller();
        match container_spec.platform {
            Some(platform) => self.get_platform(platform).await.with_context(|| {
                format!(
                    "Did not find platform {} requested by container {}.",
                    platform, container_spec.id
                )
            })?,
            None => {
                // TODO: Look for a platform this container could run on
                error!(
                    "No platform specified for container {}. We cannot handle this case yet.",
                    container_spec.id
                );
                anyhow::bail!("No platform specified for container {}", container_spec.id);
            }
        }
    }

    async fn get_platform(&self, platform: String) -> anyhow::Result<V::ContainerPlatform> {
        V::ContainerPlatform::find()
            .filter(platform.eq(&platform.clone()))
            .first(&self.app_state.db_pool)
            .await?
    }

    pub async fn watch(&self, container: V::Container) -> anyhow::Result<V::Container> {
        let container_spec = container.to_controller();
        match container_spec.platform {
            Some(platform) => {
                let platform_spec = self.get_platform(platform).await.with_context(|| {
                    format!(
                        "Did not find platform {} hosting container {}.",
                        platform, container_spec.id
                    )
                })?; // TODO: Handle gracefully. This can happen if the platform is removed/lost.
                let platform: Box<dyn ContainerPlatform<V>> =
                    create_platform_from_kind(platform_spec).await?;
                // TODO: Run as separate task?
                platform.get(container).await?
                // TODO: Handle status updates in a compatible way
            }

            None => {
                anyhow::bail!(
                    "Cannot watch container {} because it has no platform even though its status is {}.",
                    container_spec.id, container_spec.status
                );
            }
        }
    }

    pub async fn kill(&self, container: V::Container) -> anyhow::Result<V::Container> {
        let container_spec = container.to_controller();
        match container_spec.platform {
            Some(platform) => {
                let platform_spec = self.get_platform(platform).await.with_context(|| {
                    format!(
                        "Did not find platform {} hosting container {}.",
                        platform, container_spec.id
                    )
                })?; // TODO: Handle gracefully. This can happen if the platform is removed/lost.
                let platform: Box<dyn ContainerPlatform<V>> =
                    create_platform_from_kind(platform_spec).await?;
                // TODO: Run as separate task?
                platform.delete(container).await?
            }
            None => {
                anyhow::bail!(
                    "Cannot kill container {} because it has no platform even though its status is {}.",
                    container_spec.id, container_spec.status
                );
            }
        }
    }
}
