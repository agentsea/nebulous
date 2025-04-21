use super::docker::DockerPlatform;
use super::ssh::SSHConnection;

pub(crate) type EC2Platform<V> = DockerPlatform<SSHConnection<V>, V>;
