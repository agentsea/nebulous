use crate::resources::v1::containers::models::ContainerModelVersion;
use crate::resources::v1::containers::platform::ContainerPlatformStatus::DoNotSchedule;
use crate::resources::v1::containers::platform::{
    ContainerPlatform, ContainerPlatformStatus, PlatformConnection,
};

pub struct DockerPlatform<C, V>
where
    C: PlatformConnection,
    V: ContainerModelVersion,
{
    pub(crate) connection: C,
    pub(crate) version: std::marker::PhantomData<V>,
}

impl<C, V> ContainerPlatform<V> for DockerPlatform<C, V>
where
    C: PlatformConnection,
    V: ContainerModelVersion,
{
    async fn create(&self, container: V::Container) -> anyhow::Result<V::Container> {
        self.connection.run_command("docker run").await?;
        Ok(container)
    }

    async fn get(&self, id: &str) -> anyhow::Result<V::Container> {
        todo!()
    }

    async fn delete(&self, id: &str) -> anyhow::Result<()> {
        todo!()
    }

    async fn logs(&self, id: &str) -> anyhow::Result<()> {
        todo!()
    }

    async fn exec(&self, id: &str, command: &str) -> anyhow::Result<()> {
        todo!()
    }

    async fn status(&self) -> anyhow::Result<ContainerPlatformStatus> {
        Ok(DoNotSchedule)
    }
}
