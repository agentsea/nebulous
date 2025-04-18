use crate::resources::v1::containers::docker::DockerPlatform;
use crate::resources::v1::containers::ssh::SSHConnection;

type EC2Platform<V> = DockerPlatform<SSHConnection, V>;
