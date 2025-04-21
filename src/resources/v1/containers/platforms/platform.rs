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
pub trait ContainerPlatform<V: ContainerModelVersion> {
    fn validate(spec: &V::ContainerPlatform) -> anyhow::Result<()> {
        let _ = Self::from_spec(spec.clone());
        Ok(())
    }
    fn from_spec(spec: V::ContainerPlatform) -> Self;
    async fn create(&self, container: V::Container) -> anyhow::Result<V::Container>;
    async fn get(&self, id: &str) -> anyhow::Result<V::Container>;
    async fn delete(&self, id: &str) -> anyhow::Result<V::Container>;
    async fn logs(&self, id: &str) -> anyhow::Result<String>;

    // TODO: Design API
    async fn exec(&self, id: &str, command: &str) -> anyhow::Result<String>;

    // TODO: Add platform-level monitoring, status, and properties
    async fn status(&self) -> anyhow::Result<ContainerPlatformStatus>;
}

#[async_trait]
pub trait PlatformConnection<V: ContainerModelVersion> {
    fn validate(spec: &V::ContainerPlatform) -> anyhow::Result<()> {
        let _ = Self::from_spec(spec.clone());
        Ok(())
    }
    fn from_spec(spec: V::ContainerPlatform) -> Self;
    async fn connect(&self) -> anyhow::Result<()>;
    async fn disconnect(&self) -> anyhow::Result<()>;
    async fn is_connected(&self) -> bool;
}

#[async_trait]
pub trait ShellConnection {
    async fn run_command(&self, command: &str) -> anyhow::Result<String>;
}

#[async_trait]
pub trait RESTConnection {
    async fn get(&self, path: &str) -> anyhow::Result<String>;

    async fn post(&self, path: &str, body: &str) -> anyhow::Result<String>;
}
