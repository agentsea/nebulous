pub trait ContainerModelVersion {
    type Container;
    type ContainerMetaRequest;
    type ContainerHealthCheck;
    type ContainerStatus;
    type EnvVar;
    type ErrorResponse;
    type ContainerResource;
    type Port;
    type PortRequest;
    type SSHKey;

    type UpdateContainer;

    type ContainerSearch;

}
