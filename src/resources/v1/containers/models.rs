use crate::models::{
    V1AuthzConfig, V1Meter, V1ResourceMeta, V1ResourceMetaRequest, V1ResourceReference,
};
use crate::resources::containers::ContainerModelVersion;
use crate::resources::v1::volumes::models::V1VolumePath;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;

#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
pub struct V1ErrorResponse {
    #[serde(rename = "type", default = "default_error_response_type")]
    pub response_type: String,
    pub request_id: String,
    pub error: String,
    pub traceback: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
pub struct V1EnvVar {
    pub key: String,
    pub value: Option<String>,
    pub secret_name: Option<String>,
}

fn default_error_response_type() -> String {
    "ErrorResponse".to_string()
}

#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
pub struct V1ContainerMetaRequest {
    pub name: Option<String>,
    pub namespace: Option<String>,
    pub labels: Option<HashMap<String, String>>,
    pub owner_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
pub struct V1ContainerHealthCheck {
    pub interval: Option<String>,
    pub timeout: Option<String>,
    pub retries: Option<i32>,
    pub start_period: Option<String>,
    pub path: Option<String>,
    pub port: Option<i32>,
    pub protocol: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
pub struct V1ContainerRequest {
    #[serde(default = "default_container_kind")]
    pub kind: String,
    pub platform: Option<String>,
    pub metadata: Option<V1ResourceMetaRequest>,
    pub image: String,
    pub env: Option<Vec<V1EnvVar>>,
    pub command: Option<String>,
    pub args: Option<String>,
    pub volumes: Option<Vec<V1VolumePath>>,
    // pub local_volumes: Option<Vec<V1VolumePath>>,
    pub accelerators: Option<Vec<String>>,
    pub resources: Option<V1ContainerResources>,
    pub meters: Option<Vec<V1Meter>>,
    #[serde(default = "default_restart")]
    pub restart: String,
    pub queue: Option<String>,
    pub timeout: Option<String>,
    pub health_check: Option<V1ContainerHealthCheck>,
    pub ssh_keys: Option<Vec<V1SSHKey>>,
    pub ports: Option<Vec<V1PortRequest>>,
    pub proxy_port: Option<i16>,
    pub authz: Option<V1AuthzConfig>,
}

pub enum RestartPolicy {
    Always,
    Never,
}

fn default_restart() -> String {
    RestartPolicy::Never.to_string()
}

impl fmt::Display for RestartPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RestartPolicy::Always => write!(f, "Always"),
            RestartPolicy::Never => write!(f, "Never"),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
pub struct V1ContainerResources {
    pub min_cpu: Option<f64>,
    pub min_memory: Option<f64>,
    pub max_cpu: Option<f64>,
    pub max_memory: Option<f64>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
pub struct V1Port {
    pub port: u16,
    pub protocol: Option<String>,
    pub public_ip: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
pub struct V1PortRequest {
    pub port: u16,
    pub protocol: Option<String>,
    pub public: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
pub struct V1ContainerStatus {
    pub status: Option<String>,
    pub message: Option<String>,
    pub accelerator: Option<String>,
    pub public_ports: Option<Vec<V1Port>>,
    pub cost_per_hr: Option<f64>,
    pub tailnet_url: Option<String>,
    pub ready: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
pub struct V1SSHKey {
    pub public_key: Option<String>,
    pub public_key_secret: Option<String>,
    pub copy_local: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
pub struct V1Container {
    #[serde(default = "default_container_kind")]
    pub kind: String,
    pub platform: String,
    pub metadata: V1ResourceMeta,
    pub image: String,
    pub env: Option<Vec<V1EnvVar>>,
    pub command: Option<String>,
    pub args: Option<String>,
    pub volumes: Option<Vec<V1VolumePath>>,
    pub accelerators: Option<Vec<String>>,
    pub meters: Option<Vec<V1Meter>>,
    pub restart: String,
    pub queue: Option<String>,
    pub timeout: Option<String>,
    pub resources: Option<V1ContainerResources>,
    pub health_check: Option<V1ContainerHealthCheck>,
    pub status: Option<V1ContainerStatus>,
    pub ssh_keys: Option<Vec<V1SSHKey>>,
    pub ports: Option<Vec<V1PortRequest>>,
    pub proxy_port: Option<i16>,
    pub authz: Option<V1AuthzConfig>,
}

impl V1Container {
    /// Convert this container into a V1ResourceReference.
    pub fn to_resource_reference(&self) -> V1ResourceReference {
        V1ResourceReference {
            kind: self.kind.clone(),
            name: self.metadata.name.clone(),
            namespace: self.metadata.namespace.clone(),
        }
    }
}

// Add this function to provide a default kind value
fn default_container_kind() -> String {
    "Container".to_string()
}

#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
pub struct V1Containers {
    pub containers: Vec<V1Container>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
pub struct V1UpdateContainer {
    pub image: Option<String>,
    pub env: Option<Vec<V1EnvVar>>,
    pub command: Option<String>,
    pub args: Option<String>,
    pub volumes: Option<Vec<V1VolumePath>>,
    pub accelerators: Option<Vec<String>>,
    pub labels: Option<HashMap<String, String>>,
    pub cpu_request: Option<String>,
    pub memory_request: Option<String>,
    pub platform: Option<String>,
    pub health_check: Option<V1ContainerHealthCheck>,
    pub meters: Option<Vec<V1Meter>>,
    pub restart: Option<String>,
    pub queue: Option<String>,
    pub timeout: Option<String>,
    pub resources: Option<V1ContainerResources>,
    pub proxy_port: Option<i16>,
    pub no_delete: Option<bool>,
    pub authz: Option<V1AuthzConfig>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
pub struct V1ContainerSearch {
    pub namespace: Option<String>,
    pub image: Option<String>,
    pub env: Option<Vec<V1EnvVar>>,
    pub command: Option<String>,
    pub args: Option<String>,
    pub volumes: Option<Vec<V1VolumePath>>,
    pub accelerators: Option<Vec<String>>,
    pub labels: Option<HashMap<String, String>>,
    pub cpu_request: Option<String>,
    pub memory_request: Option<String>,
    pub platform: Option<String>,
    pub health_check: Option<V1ContainerHealthCheck>,
    pub meters: Option<Vec<V1Meter>>,
    pub restart: Option<String>,
    pub queue: Option<String>,
    pub timeout: Option<String>,
    pub resources: Option<V1ContainerResources>,
    pub proxy_port: Option<i16>,
    pub authz: Option<V1AuthzConfig>,
}

pub struct V1;

impl ContainerModelVersion for V1 {
    type Container = V1Container;
    type ContainerMetaRequest = V1ContainerMetaRequest;
    type ContainerHealthCheck = V1ContainerHealthCheck;
    type ContainerStatus = V1ContainerStatus;
    type EnvVar = V1EnvVar;
    type ErrorResponse = V1ErrorResponse;
    type ContainerResource = V1ContainerResources;
    type Port = V1Port;
    type PortRequest = V1PortRequest;
    type SSHKey = V1SSHKey;
    type UpdateContainer = V1UpdateContainer;
    type ContainerSearch = V1ContainerSearch;
}
