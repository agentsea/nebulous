use super::platform::ContainerPlatform;
use super::{
    docker::LocalDockerPlatform, docker::RemoteDockerPlatform, ec2::EC2Platform, gce::GCEPlatform,
};
use crate::resources::v1::containers::models::ContainerModelVersion;

#[derive(Debug)]
enum PlatformKind {
    LocalDocker,
    RemoteDocker,
    EC2,
    GCE,
}

impl std::str::FromStr for PlatformKind {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "local-docker" => Ok(Self::LocalDocker),
            "remote-docker" => Ok(Self::RemoteDocker),
            "ec2" => Ok(Self::EC2),
            "gce" => Ok(Self::GCE),
            _ => Err(anyhow::anyhow!("Unknown platform kind: {}", s)),
        }
    }
}

pub(crate) fn create_platform_from_kind<V: ContainerModelVersion>(
    kind: &str,
    spec: V::ContainerPlatform,
) -> anyhow::Result<impl ContainerPlatform<V>> {
    let kind = kind.parse::<PlatformKind>()?;

    match kind {
        PlatformKind::LocalDocker => Ok(LocalDockerPlatform::<V>::from_spec(spec)),
        PlatformKind::RemoteDocker => Ok(RemoteDockerPlatform::<V>::from_spec(spec)),
        PlatformKind::EC2 => Ok(EC2Platform::<V>::from_spec(spec)),
        PlatformKind::GCE => Ok(GCEPlatform::<V>::from_spec(spec)),
    }
}
