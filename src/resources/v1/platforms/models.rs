use crate::resources::platforms::PlatformModelVersion;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
pub struct V1ContainerPlatform {}

pub struct V1;

impl PlatformModelVersion for V1 {
    type ContainerPlatform = V1ContainerPlatform;
}

