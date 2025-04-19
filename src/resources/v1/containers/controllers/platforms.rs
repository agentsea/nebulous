use super::super::platforms::factory::create_platform_from_kind;
use crate::resources::v1::containers::models::ContainerModelVersion;
use crate::state::AppState;
use std::sync::Arc;
use tracing::{info, span, Instrument, Level};

pub struct ContainerPlatformController<V>
where
    V: ContainerModelVersion,
{
    app_state: Arc<AppState>,
}

enum ContainerPlatformStatus {
    Started,
    Initializing,
    Available,
    Unavailable,
    Stopped,
    Terminating,
    Terminated,
}

impl<V> ContainerPlatformController<V>
where
    V: ContainerModelVersion,
{
    pub fn new(app_state: Arc<AppState>) -> Self {
        Self { app_state }
    }

    pub async fn reconcile(&self) {
        let span = span!(Level::INFO, "ContainerPlatformController");

        async {
            info!("Starting reconciliation process");
            let paginator = V::ContainerPlatform::find()
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

    pub async fn handle(&self, mut platform: V::ContainerPlatform) {
        let platform_status: ContainerPlatformStatus = call();

        match platform_status {
            ContainerPlatformStatus::Started => {
                info!("Platform is started");
                self.initialize(platform).await;
            }
            ContainerPlatformStatus::Initializing
            | ContainerPlatformStatus::Available
            | ContainerPlatformStatus::Unavailable
            | ContainerPlatformStatus::Terminating => {
                info!("Platform is initialized");
                self.watch(platform).await;
            }
            ContainerPlatformStatus::Stopped => {
                info!("Platform is stopped");
                self.terminate(platform).await;
            }
            ContainerPlatformStatus::Terminated => {}
        }
    }

    pub async fn initialize(platform: V::ContainerPlatform) {
        let kind = platform.kind.clone();
        let platform = create_platform_from_kind(kind, platform).await;
    }

    pub async fn watch(platform: V::ContainerPlatform) {
        let kind = platform.kind.clone();
        let platform = create_platform_from_kind(kind, platform).await;
    }

    pub async fn terminate(platform: V::ContainerPlatform) {
        let kind = platform.kind.clone();
        let platform = create_platform_from_kind(kind, platform).await;
    }
}
