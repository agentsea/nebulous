
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
use std::collections::HashMap;

use crate::models::{V1AuthzConfig, V1Meter};
use crate::resources::v1::containers::models::{
    V1Container, V1ContainerHealthCheck, V1ContainerResources, V1ContainerStatus, V1EnvVar,
    V1PortRequest, V1SSHKey,
};
use crate::resources::v1::volumes::models::V1VolumePath;


#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "containers")]
pub struct Model {
    #[sea_orm(primary_key, column_type = "Text", auto_increment = false)]
    pub id: String,
    pub version: i32,
    pub namespace: String,
    pub name: String,
    #[sea_orm(unique, column_type = "Text")]
    pub full_name: String,
    pub owner: String,
    pub owner_ref: Option<String>,
    pub image: String,
    pub env: Option<Json>,
    pub volumes: Option<Json>,
    pub local_volumes: Option<Json>,
    pub accelerators: Option<Vec<String>>,
    pub cpu_request: Option<String>,
    pub memory_request: Option<String>,
    pub status: Option<Json>,
    pub platform: Option<String>,
    pub platforms: Option<Vec<String>>,
    pub resource_name: Option<String>,
    pub resource_namespace: Option<String>,
    pub resource_cost_per_hr: Option<f64>,
    pub command: Option<String>,
    pub args: Option<String>,
    pub labels: Option<Json>,
    pub meters: Option<Json>,
    pub queue: Option<String>,
    pub ports: Option<Json>,
    pub proxy_port: Option<i16>,
    pub timeout: Option<String>,
    pub resources: Option<Json>,
    pub health_check: Option<Json>,
    pub restart: String,
    pub authz: Option<Json>,
    pub public_addr: Option<String>,
    pub tailnet_ip: Option<String>,
    pub created_by: Option<String>,
    pub desired_status: Option<String>,
    pub controller_data: Option<Json>,
    pub container_user: Option<String>,
    pub ssh_keys: Option<Json>,
    pub updated_at: DateTimeWithTimeZone,
    pub created_at: DateTimeWithTimeZone,
}

// The Relation enum is required, even if empty.
#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

// The ActiveModelBehavior is required, even if empty.
impl ActiveModelBehavior for ActiveModel {}

impl Model {
    /// Attempt to parse `env` into a vector of `V1EnvVar`.
    pub fn parse_env(&self) -> Result<Option<Vec<V1EnvVar>>, serde_json::Error> {
        if let Some(json_value) = &self.env {
            serde_json::from_value(json_value.clone()).map(Some)
        } else {
            Ok(None)
        }
    }

    /// Attempt to parse `volumes` into a vector of `V1VolumePath`.
    pub fn parse_volumes(&self) -> Result<Option<Vec<V1VolumePath>>, serde_json::Error> {
        if let Some(json_value) = &self.volumes {
            serde_json::from_value(json_value.clone()).map(Some)
        } else {
            Ok(None)
        }
    }

    /// Attempt to parse `status` into a `V1ContainerStatus`.
    pub fn parse_status(&self) -> Result<Option<V1ContainerStatus>, serde_json::Error> {
        if let Some(json_value) = &self.status {
            serde_json::from_value(json_value.clone()).map(Some)
        } else {
            Ok(None)
        }
    }

    /// Attempt to parse `labels` into a `HashMap<String, String>`.
    pub fn parse_labels(&self) -> Result<Option<HashMap<String, String>>, serde_json::Error> {
        if let Some(json_value) = &self.labels {
            serde_json::from_value(json_value.clone()).map(Some)
        } else {
            Ok(None)
        }
    }

    /// Attempt to parse `meters` into a vector of `V1Meter`.
    pub fn parse_meters(&self) -> Result<Option<Vec<V1Meter>>, serde_json::Error> {
        if let Some(json_value) = &self.meters {
            serde_json::from_value(json_value.clone()).map(Some)
        } else {
            Ok(None)
        }
    }

    /// Attempt to parse `controller_data` into any desired struct T that implements Deserialize.
    /// For example, you could parse to a generic `serde_json::Value` or a custom struct.
    pub fn parse_controller_data<T: serde::de::DeserializeOwned>(
        &self,
    ) -> Result<Option<T>, serde_json::Error> {
        if let Some(json_value) = &self.controller_data {
            serde_json::from_value(json_value.clone()).map(Some)
        } else {
            Ok(None)
        }
    }

    /// Attempt to parse `resources` into a `V1ContainerResources`.
    pub fn parse_resources(&self) -> Result<Option<V1ContainerResources>, serde_json::Error> {
        if let Some(json_value) = &self.resources {
            serde_json::from_value(json_value.clone()).map(Some)
        } else {
            Ok(None)
        }
    }

    /// Attempt to parse `ssh_keys` into a vector of `V1SSHKey`.
    pub fn parse_ssh_keys(&self) -> Result<Option<Vec<V1SSHKey>>, serde_json::Error> {
        if let Some(json_value) = &self.ssh_keys {
            serde_json::from_value(json_value.clone()).map(Some)
        } else {
            Ok(None)
        }
    }

    /// Attempt to parse `ports` into a vector of `V1PortRequest`.
    pub fn parse_ports(&self) -> Result<Option<Vec<V1PortRequest>>, serde_json::Error> {
        if let Some(json_value) = &self.ports {
            serde_json::from_value(json_value.clone()).map(Some)
        } else {
            Ok(None)
        }
    }

    /// Attempt to parse `authz` into a `V1AuthzConfig`.
    pub fn parse_authz(&self) -> Result<Option<V1AuthzConfig>, serde_json::Error> {
        if let Some(json_value) = &self.authz {
            serde_json::from_value(json_value.clone()).map(Some)
        } else {
            Ok(None)
        }
    }

    /// Attempt to parse `health_check` into a `V1ContainerHealthCheck`.
    pub fn parse_health_check(&self) -> Result<Option<V1ContainerHealthCheck>, serde_json::Error> {
        if let Some(json_value) = &self.health_check {
            serde_json::from_value(json_value.clone()).map(Some)
        } else {
            Ok(None)
        }
    }

}
