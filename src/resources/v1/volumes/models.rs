use crate::models::{V1ResourceMeta, V1ResourceMetaRequest};
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
pub struct V1Volume {
    #[serde(default = "default_volume_kind")]
    pub kind: String,
    pub metadata: V1ResourceMeta,
    pub source: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
pub struct V1VolumeRequest {
    pub metadata: V1ResourceMetaRequest,
    pub source: String,
}

fn default_volume_kind() -> String {
    "Volume".to_string()
}

#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
pub struct V1VolumePath {
    pub source: String,
    pub dest: String,
    #[serde(default)]
    pub resync: bool,
    #[serde(default = "default_continuous")]
    pub continuous: bool,
    #[serde(default = "default_volume_driver")]
    pub driver: V1VolumeDriver,
}

fn default_volume_driver() -> V1VolumeDriver {
    V1VolumeDriver::RcloneSync
}

fn default_continuous() -> bool {
    false
}

// Add this function to provide a default cache directory
fn default_cache_dir() -> String {
    // Use a sensible default location for the cache
    format!("/nebu/cache")
}

#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
pub struct V1VolumeConfig {
    pub paths: Vec<V1VolumePath>,
    #[serde(default = "default_cache_dir")]
    pub cache_dir: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
pub enum V1VolumeDriver {
    #[default]
    RcloneSync,
    RcloneCopy,
    RcloneBisync,
    RcloneMount,
}

impl fmt::Display for V1VolumeDriver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            V1VolumeDriver::RcloneSync => write!(f, "rclone-sync"),
            V1VolumeDriver::RcloneCopy => write!(f, "rclone-copy"),
            V1VolumeDriver::RcloneBisync => write!(f, "rclone-bisync"),
            V1VolumeDriver::RcloneMount => write!(f, "rclone-mount"),
        }
    }
}
