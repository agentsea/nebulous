use super::docker::DockerPlatform;
use super::platform::{ContainerPlatform, ContainerPlatformStatus};
use super::ssh::SSHConnection;
use crate::resources::v1::containers::models::ContainerModelVersion;

pub struct GCEPlatform<V: ContainerModelVersion> {
    inner: DockerPlatform<SSHConnection, V>,
    gce_zone: String,
    gce_instance: String,
}

impl<V: ContainerModelVersion> GCEPlatform<V> {
    pub fn new(gce_zone: String, gce_instance: String, connection: SSHConnection) -> Self {
        let inner = DockerPlatform {
            connection,
            version: std::marker::PhantomData,
        };
        GCEPlatform {
            inner,
            gce_zone,
            gce_instance,
        }
    }
}

impl<V: ContainerModelVersion> ContainerPlatform<V> for GCEPlatform<V> {
    async fn create(&self, container: V::Container) -> anyhow::Result<V::Container> {
        self.inner.create(container).await
    }

    async fn get(&self, id: &str) -> anyhow::Result<V::Container> {
        self.inner.get(id).await
    }

    async fn delete(&self, id: &str) -> anyhow::Result<()> {
        self.inner.delete(id).await
    }

    async fn logs(&self, id: &str) -> anyhow::Result<()> {
        todo!()
    }

    async fn exec(&self, id: &str, command: &str) -> anyhow::Result<()> {
        todo!()
    }

    async fn platform_status(&self) -> anyhow::Result<ContainerPlatformStatus> {
        todo!()
    }
}
