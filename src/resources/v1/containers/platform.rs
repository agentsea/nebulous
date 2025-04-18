use crate::resources::v1::containers::models::ContainerModelVersion;
use async_trait::async_trait;

pub enum ContainerPlatformStatus {
    Initializing,
    Ready,
    Unavailable,
    DoNotSchedule,
    Terminating,
}

#[async_trait]
pub trait ContainerPlatform<T: ContainerModelVersion> {
    async fn create(&self, container: T::Container) -> anyhow::Result<T::Container>;
    async fn get(&self, id: &str) -> anyhow::Result<T::Container>;
    async fn delete(&self, id: &str) -> anyhow::Result<()>;
    async fn logs(&self, id: &str) -> anyhow::Result<()>;

    // TODO: Design API
    async fn exec(&self, id: &str, command: &str) -> anyhow::Result<()>;

    // TODO: Add platform-level monitoring, status, and properties
    async fn status(&self) -> anyhow::Result<ContainerPlatformStatus>;
}

#[async_trait]
pub trait PlatformConnection {
    async fn connect(&self) -> anyhow::Result<()>;
    async fn disconnect(&self) -> anyhow::Result<()>;
    async fn is_connected(&self) -> bool;
    async fn run_command(&self, command: &str) -> anyhow::Result<String>;
}
