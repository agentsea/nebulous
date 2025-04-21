use super::docker::DockerPlatform;
use super::platform::{ContainerPlatform, ContainerPlatformStatus};
use super::ssh::SSHConnection;
use crate::resources::v1::containers::models::ContainerModelVersion;

pub struct GCEPlatform<V: ContainerModelVersion> {
    inner: DockerPlatform<SSHConnection<V>, V>,
    gce_zone: String,
    gce_instance: String,
}

impl<V: ContainerModelVersion> GCEPlatform<V> {
    pub fn new(gce_zone: String, gce_instance: String, connection: SSHConnection<V>) -> Self {
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
    fn validate(spec: V::ContainerPlatform) -> anyhow::Result<()> {
        if spec.gce.gce_zone.is_empty() {
            return Err(anyhow::anyhow!("GCE zone is required"));
        }
        if spec.gce.gce_instance.is_empty() {
            return Err(anyhow::anyhow!("GCE instance is required"));
        }
        Ok(())
    }
    fn from_spec(spec: V::ContainerPlatform) -> Self {
        let gce_zone = spec.gce.gce_zone;
        let gce_instance = spec.gce.gce_instance;
        let connection = SSHConnection::from_spec(spec);
        GCEPlatform::new(gce_zone, gce_instance, connection)
    }
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

    async fn status(&self) -> anyhow::Result<ContainerPlatformStatus> {
        todo!()
    }
}
