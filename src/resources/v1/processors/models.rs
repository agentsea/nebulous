use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::models::{V1ResourceMeta, V1ResourceMetaRequest, V1ResourceReference};
use crate::resources::v1::containers::models::V1ContainerRequest;

#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
pub struct V1ProcessorStatus {
    pub status: Option<String>,
    pub message: Option<String>,
    pub pressure: Option<i32>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
pub struct V1ScaleUp {
    pub above_pressure: Option<i32>,
    pub duration: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
pub struct V1ScaleDown {
    pub below_pressure: Option<i32>,
    pub duration: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
pub struct V1ScaleZero {
    pub duration: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
pub struct V1Scale {
    pub up: Option<V1ScaleUp>,
    pub down: Option<V1ScaleDown>,
    pub zero: Option<V1ScaleZero>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
pub struct V1Processor {
    #[serde(default = "default_processor_kind")]
    pub kind: String,
    pub metadata: V1ResourceMeta,
    pub container: Option<V1ContainerRequest>,
    pub stream: String,
    pub schema: Option<Value>,
    pub common_schema: Option<String>,
    pub min_replicas: Option<i32>,
    pub max_replicas: Option<i32>,
    pub scale: Option<V1Scale>,
    pub status: Option<V1ProcessorStatus>,
}

impl V1Processor {
    /// Convert this processor into a V1ResourceReference.
    pub fn to_resource_reference(&self) -> V1ResourceReference {
        V1ResourceReference {
            kind: self.kind.clone(),
            name: self.metadata.name.clone(),
            namespace: self.metadata.namespace.clone(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
pub struct V1ProcessorRequest {
    #[serde(default = "default_processor_kind")]
    pub kind: String,
    pub metadata: V1ResourceMetaRequest,
    pub container: Option<V1ContainerRequest>,
    pub schema: Option<Value>,
    pub common_schema: Option<String>,
    pub min_replicas: Option<i32>,
    pub max_replicas: Option<i32>,
    pub scale: Option<V1Scale>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct V1Processors {
    pub processors: Vec<V1Processor>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct V1ProcessorScaleRequest {
    pub replicas: Option<i32>,
    pub min_replicas: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct V1UpdateProcessor {
    pub kind: Option<String>,
    pub metadata: Option<V1ResourceMetaRequest>,
    pub container: Option<V1ContainerRequest>,
    pub stream: Option<String>,
    pub min_replicas: Option<i32>,
    pub max_replicas: Option<i32>,
    pub scale: Option<V1Scale>,
    pub schema: Option<Value>,
    pub common_schema: Option<String>,
    pub no_delete: Option<bool>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct V1ReadStreamRequest {
    pub consumer_group: String,
    #[serde(default = "default_max_records")]
    pub max_records: u64,
    #[serde(default = "default_wait_time_ms")]
    pub wait_time_ms: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct V1ProcessorHealthResponse {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

fn default_processor_kind() -> String {
    "Processor".to_string()
}

fn default_max_records() -> u64 {
    10
}

fn default_wait_time_ms() -> u64 {
    1000
}
