use crate::resources::containers::ContainerModelVersion;
use crate::resources::platforms::PlatformModelVersion;
use async_trait::async_trait;

pub enum ContainerPlatformStatus {
    Initializing,
    Ready,
    Unavailable,
    DoNotSchedule,
    Terminating,
}


pub trait ContainerPlatformBuilder<V: PlatformModelVersion> {

    fn validate(spec: &V::ContainerPlatform) -> anyhow::Result<()> {
        let _ = Self::from_spec(spec.clone());
        Ok(())
    }

    fn from_spec(spec: V::ContainerPlatform) -> Self;
}


#[async_trait]
pub trait ContainerPlatform<C: ContainerModelVersion> {
    async fn create(&self, container: C::Container) -> anyhow::Result<C::Container>;
    async fn get(&self, id: &str) -> anyhow::Result<C::Container>;
    async fn delete(&self, id: &str) -> anyhow::Result<C::Container>;
    async fn logs(&self, id: &str) -> anyhow::Result<String>;

    // TODO: Design API
    async fn exec(&self, id: &str, command: &str) -> anyhow::Result<String>;

    // TODO: Add platform-level monitoring, status, and properties
    async fn status(&self) -> anyhow::Result<ContainerPlatformStatus>;
}

#[async_trait]
pub trait PlatformConnection<V: PlatformModelVersion> {
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
