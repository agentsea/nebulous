use crate::resources::v1::containers::models::ContainerModelVersion;
use crate::resources::v1::containers::platforms::platform::{
    ContainerPlatform, ContainerPlatformStatus, PlatformConnection, RESTConnection,
};

pub struct NebulousPlatform<C, V>
where
    C: PlatformConnection<V> + RESTConnection,
    V: ContainerModelVersion,
{
    pub(crate) version: std::marker::PhantomData<V>,
    connection: C,
}

impl<C, V> ContainerPlatform<V> for NebulousPlatform<C, V>
where
    C: PlatformConnection<V> + RESTConnection,
    V: ContainerModelVersion,
{
    fn from_spec(spec: V::ContainerPlatform) -> Self {
        todo!()
    }

    async fn create(&self, container: V::Container) -> anyhow::Result<V::Container> {
        todo!()
    }

    async fn get(&self, id: &str) -> anyhow::Result<V::Container> {
        todo!()
    }

    async fn delete(&self, id: &str) -> anyhow::Result<V::Container> {
        todo!()
    }

    async fn logs(&self, id: &str) -> anyhow::Result<String> {
        todo!()
    }

    async fn exec(&self, id: &str, command: &str) -> anyhow::Result<String> {
        todo!()
    }

    async fn status(&self) -> anyhow::Result<ContainerPlatformStatus> {
        todo!()
    }
}
