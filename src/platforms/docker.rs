use super::local::LocalShell;
use super::platform::{
    ContainerPlatform, ContainerPlatformBuilder, ContainerPlatformStatus, PlatformConnection,
    ShellConnection,
};
use crate::resources::v1::containers::models::v1::V1Container;
use crate::resources::v1::containers::models::ContainerModelVersion;
use crate::resources::v1::containers::platforms::ssh::SSHConnection;
use async_trait::async_trait;
use serde::Deserialize;

pub struct DockerPlatform<C, V>
where
    C: PlatformConnection<V> + ShellConnection,
    V: ContainerModelVersion,
{
    pub(crate) connection: C,
    pub(crate) version: std::marker::PhantomData<V>,
}

struct DockerContainer {
    pub image: String,
}

trait ToDockerContainer {
    fn to_docker(&self) -> DockerContainer;
}

trait FromDockerContainer {
    fn from_docker(docker_container: DockerContainer) -> Self;
}

impl ToDockerContainer for V1Container {
    fn to_docker(&self) -> DockerContainer {
        DockerContainer {
            image: self.image.clone(),
        }
    }
}

impl FromDockerContainer for V1Container {
    fn from_docker(docker_container: DockerContainer) -> Self {
        V1Container {
            image: docker_container.image,
            ..Default::default()
        }
    }
}

#[allow(non_snake_case)]
#[derive(Debug, Deserialize)]
struct DockerInspect {
    pub Id: String,
    pub RepoTags: Option<Vec<String>>,
    pub RepoDigests: Option<Vec<String>>,
    pub Parent: Option<String>,
    pub Comment: Option<String>,
    pub Created: Option<String>,
    pub DockerVersion: Option<String>,
    pub Author: Option<String>,
    pub Config: Option<DockerInspectConfig>,
    pub Architecture: Option<String>,
    pub Os: Option<String>,
    pub Size: Option<u64>,
    pub GraphDriver: Option<DockerInspectGraphDriver>,
    pub RootFS: Option<DockerInspectRootFS>,
    pub Metadata: Option<DockerInspectMetadata>,
}

#[allow(non_snake_case)]
#[derive(Debug, Deserialize)]
struct DockerInspectConfig {
    pub Hostname: Option<String>,
    pub Domainname: Option<String>,
    pub User: Option<String>,
    pub AttachStdin: Option<bool>,
    pub AttachStdout: Option<bool>,
    pub AttachStderr: Option<bool>,
    pub ExposedPorts: Option<std::collections::HashMap<String, serde_json::Value>>,
    pub Tty: Option<bool>,
    pub OpenStdin: Option<bool>,
    pub StdinOnce: Option<bool>,
    pub Env: Option<Vec<String>>,
    pub Cmd: Option<Vec<String>>,
    pub Image: Option<String>,
    pub Volumes: Option<serde_json::Value>,
    pub WorkingDir: Option<String>,
    pub Entrypoint: Option<Vec<String>>,
    pub OnBuild: Option<serde_json::Value>,
    pub Labels: Option<std::collections::HashMap<String, String>>,
    pub StopSignal: Option<String>,
}

#[allow(non_snake_case)]
#[derive(Debug, Deserialize)]
struct DockerInspectGraphDriver {
    pub Data: Option<DockerInspectGraphDriverData>,
    pub Name: Option<String>,
}

#[allow(non_snake_case)]
#[derive(Debug, Deserialize)]
struct DockerInspectGraphDriverData {
    pub LowerDir: Option<String>,
    pub MergedDir: Option<String>,
    pub UpperDir: Option<String>,
    pub WorkDir: Option<String>,
}

#[allow(non_snake_case)]
#[derive(Debug, Deserialize)]
struct DockerInspectRootFS {
    pub Type: Option<String>,
    pub Layers: Option<Vec<String>>,
}

#[allow(non_snake_case)]
#[derive(Debug, Deserialize)]
struct DockerInspectMetadata {
    pub LastTagTime: Option<String>,
}

impl ToDockerContainer for DockerInspect {
    fn to_docker(&self) -> DockerContainer {
        DockerContainer {
            image: self
                .Config
                .as_ref()
                .and_then(|config| config.Image.clone())
                .unwrap_or_default(),
        }
    }
}

impl<C, V> ContainerPlatformBuilder<V> for DockerPlatform<C, V>
where
    C: PlatformConnection<V> + ShellConnection,
    V: ContainerModelVersion,
{
    fn from_spec(spec: V::ContainerPlatform) -> Self {
        let connection = C::from_spec(spec);
        DockerPlatform {
            connection,
            version: std::marker::PhantomData,
        }
    }
}

#[async_trait]
impl<C, V> ContainerPlatform<V> for DockerPlatform<C, V>
where
    C: PlatformConnection<V> + ShellConnection,
    V: ContainerModelVersion,
    V::Container: FromDockerContainer + ToDockerContainer,
{
    async fn create(&self, container: V::Container) -> anyhow::Result<V::Container> {
        let docker_container = container.to_docker();
        let command = format!("docker run -d {}", docker_container.image);
        let id = self.connection.run_command(&command).await?;
        Ok(container)
    }

    async fn get(&self, id: &str) -> anyhow::Result<V::Container> {
        let command = format!("docker inspect {}", id);
        let output = self.connection.run_command(&command).await?;
        let docker_inspect = serde_json::from_str::<DockerInspect>(&output)?;
        let docker_container = docker_inspect.to_docker();
        let container = V::Container::from_docker(docker_container);
        Ok(container)
    }

    async fn delete(&self, id: &str) -> anyhow::Result<V::Container> {
        let command = format!("docker rm -f {}", id);
        let output = self.connection.run_command(&command).await?;
        if !output.is_empty() {
            return Err(anyhow::anyhow!("Failed to delete container: {}", output));
        }
        Ok(())
    }

    async fn logs(&self, id: &str) -> anyhow::Result<String> {
        let command = format!("docker logs --tail 1000 {}", id);
        let logs = self.connection.run_command(&command).await?;
        Ok(logs)
    }

    async fn exec(&self, id: &str, command: &str) -> anyhow::Result<String> {
        let command = format!("docker exec -it {} {}", id, command);
        let output = self.connection.run_command(&command).await?;
        Ok(output)
    }

    async fn status(&self) -> anyhow::Result<ContainerPlatformStatus> {
        // TODO: Check capacity
        if self.connection.is_connected().await? {
            Ok(ContainerPlatformStatus::Ready)
        } else {
            Ok(ContainerPlatformStatus::DoNotSchedule)
        }
    }
}

pub(crate) type LocalDockerPlatform<V> = DockerPlatform<LocalShell<V>, V>;
pub(crate) type RemoteDockerPlatform<V> = DockerPlatform<SSHConnection<V>, V>;
