use crate::models::V1ResourceMeta;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
pub struct V1Namespace {
    #[serde(default = "default_namespace_kind")]
    pub kind: String,
    pub metadata: V1ResourceMeta,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
pub struct V1NamespaceRequest {
    pub metadata: V1NamespaceMetaRequest,
}

fn default_namespace_kind() -> String {
    "Namespace".to_string()
}

#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
pub struct V1NamespaceMetaRequest {
    pub name: String,
    pub labels: Option<HashMap<String, String>>,
    pub owner: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
pub struct V1Namespaces {
    pub namespaces: Vec<V1Namespace>,
}
