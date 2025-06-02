use async_trait::async_trait;
use crate::resources::v1::containers::models::v1::V1ContainerPlatform;
use crate::resources::v1::containers::models::ContainerModelVersion;
use crate::resources::v1::containers::platforms::platform::{PlatformConnection, RESTConnection};

pub struct HTTPConnection<V: ContainerModelVersion> {
    version: std::marker::PhantomData<V>,
    host: String,
    health_check_path: String,
}

trait GetHttpConnectionDetails {
    fn host(&self) -> String;
    fn health_check_path(&self) -> String;
}

impl GetHttpConnectionDetails for V1ContainerPlatform {
    fn host(&self) -> String {
        todo!()
    }

    fn health_check_path(&self) -> String {
        todo!()
    }
}

#[async_trait]
impl<V> PlatformConnection<V> for HTTPConnection<V>
where
    V: ContainerModelVersion,
{
    fn from_spec(spec: V::ContainerPlatform) -> Self {
        todo!()
    }

    async fn connect(&self) -> anyhow::Result<()> {
        self.is_connected().await?;
        Ok(())
    }

    async fn disconnect(&self) -> anyhow::Result<()> {
        Ok(())
    }

    async fn is_connected(&self) -> bool {
        match self.get(&self.health_check_path).await {
            Ok(_) => true,
            Err(_) => false,
        }
    }
}

#[async_trait]
impl<V> RESTConnection for HTTPConnection<V>
where
    V: ContainerModelVersion,
{
    async fn get(&self, path: &str) -> anyhow::Result<String> {
        let client = reqwest::Client::new();
        client
            .get(format!("{}/{}", self.host, path))
            .send()
            .await?
            .text()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to read response: {}", e))
    }

    async fn post(&self, path: &str, body: &str) -> anyhow::Result<String> {
        let client = reqwest::Client::new();
        client
            .post(format!("{}/{}", self.host, path))
            .body(body.to_string())
            .send()
            .await?
            .text()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to read response: {}", e))
    }
}
