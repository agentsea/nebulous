use crate::platforms::factory::create_platform_from_kind;
use crate::resources::containers::ContainerModelVersion;
use crate::resources::platforms::PlatformModelVersion;
use crate::state::AppState;
use std::sync::Arc;
use tracing::{info, span, Instrument, Level};
use crate::resources::v1::containers::models::{V1Container};
use crate::resources::v1::platforms::models::{V1ContainerPlatform};

pub struct ContainerPlatformController<V>
where
    V: ContainerModelVersion,
{
    version: std::marker::PhantomData<V>,
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

trait PlatformStatus {
    fn get_status(&self) -> ContainerPlatformStatus;
    fn set_status(&mut self, status: ContainerPlatformStatus);

}

impl PlatformStatus for V1ContainerPlatform {
    fn get_status(&self) -> ContainerPlatformStatus {
        ContainerPlatformStatus::Available
    }

    fn set_status(&mut self, status: ContainerPlatformStatus) {

    }
}

impl<V> ContainerPlatformController<V>
where
    V: PlatformModelVersion,
    V::ContainerPlatform: PlatformStatus
{
    pub fn new(app_state: Arc<AppState>) -> Self {
        Self { version: std::marker::PhantomData, app_state }
    }

    pub async fn reconcile(&self) -> anyhow::Result<()> {
        let span = span!(Level::INFO, "ContainerPlatformController");

        async {
            info!("Starting reconciliation process");
            let paginator = V::ContainerPlatform::find()
                .paginate(&self.app_state.db_pool, 100)
                .await;
            let total_pages = paginator.num_pages().await;

            for page in 1..=total_pages {
                let platforms = paginator.fetch_page(page).await;
                for mut platform in platforms {
                    self.handle(platform).await.expect("TODO: panic message");
                }
            }
            info!("Finished reconciliation process");
        }
        .instrument(span)
        .await;

        Ok(())
    }

    pub async fn handle(&self, mut platform: V::ContainerPlatform) -> anyhow::Result<()> {
        let platform_status: ContainerPlatformStatus = platform.get_status();

        match platform_status {
            ContainerPlatformStatus::Started => {
                info!("Platform is started");
                self.initialize(platform).await?;
            }
            ContainerPlatformStatus::Initializing
            | ContainerPlatformStatus::Available
            | ContainerPlatformStatus::Unavailable
            | ContainerPlatformStatus::Terminating => {
                info!("Platform is active, getting status...");
                self.watch(platform).await?;
            }
            ContainerPlatformStatus::Stopped => {
                info!("Platform is stopped");
                self.terminate(platform).await?;
            }
            ContainerPlatformStatus::Terminated => {}
        }

        Ok(())
    }

    pub async fn initialize(&self, platform: V::ContainerPlatform) -> anyhow::Result<V::ContainerPlatform> {
        let platform = create_platform_from_kind(platform).await?;
        platform
    }

    pub async fn watch(&self, platform: V::ContainerPlatform) -> anyhow::Result<V::ContainerPlatform> {
        let platform = create_platform_from_kind(platform).await?;
        platform
    }

    pub async fn terminate(&self, platform: V::ContainerPlatform) -> anyhow::Result<V::ContainerPlatform> {
        let platform = create_platform_from_kind(platform).await?;
        platform
    }
}
