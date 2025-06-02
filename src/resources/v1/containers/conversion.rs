use crate::conversion::{FromEntity, ToEntity};
use crate::entities;
use crate::resources::v1::containers::models::V1Container;

impl FromEntity<entities::containers::Model> for V1Container
{
    /// Construct a full V1Container from the current model row.
    /// Returns a serde_json Error if any JSON parsing in subfields fails.
    fn from_entity(entity: entities::containers::Model) -> Result<V1Container, serde_json::Error> {
        let env = entity.parse_env()?;
        let volumes = entity.parse_volumes()?;
        let status = entity.parse_status()?;
        let labels = entity.parse_labels()?;
        let meters = entity.parse_meters()?;
        let resources = entity.parse_resources()?;
        let ssh_keys = entity.parse_ssh_keys()?;
        let ports = entity.parse_ports()?;
        let authz = entity.parse_authz()?;
        let health_check = entity.parse_health_check()?;

        // Build metadata; fill with defaults or unwrap as needed
        let metadata = crate::models::V1ResourceMeta {
            name: entity.name.clone(),
            namespace: entity.namespace.clone(),
            id: entity.id.clone(),
            owner: entity.owner.clone(),
            owner_ref: entity.owner_ref.clone(),
            created_at: entity.created_at.timestamp(),
            updated_at: entity.updated_at.timestamp(),
            created_by: entity.created_by.clone().unwrap_or_default(),
            labels,
        };

        // Construct final V1Container
        let container = V1Container {
            kind: "Container".to_owned(), // or use default_container_kind() if needed
            platform: entity.platform.clone().unwrap_or_default(),
            metadata,
            image: entity.image.clone(),
            env,
            command: entity.command.clone(),
            args: entity.args.clone(),
            volumes,
            accelerators: entity.accelerators.clone(),
            meters,
            restart: entity.restart.clone(),
            queue: entity.queue.clone(),
            timeout: entity.timeout.clone(),
            status,
            resources,
            health_check,
            ssh_keys,
            ports: ports.clone(),
            proxy_port: entity.proxy_port.clone(),
            authz,
        };

        Ok(container)
    }
}


impl ToEntity<entities::containers::Model> for V1Container {
    /// Convert a V1Container into a Model.
    /// Returns a serde_json Error if any JSON parsing in subfields fails.
    fn to_entity(&self) -> Result<entities::containers::Model, serde_json::Error> {
        // Convert the metadata
        let metadata = self.metadata.clone();

        // Convert the container
        let model = entities::containers::Model {
            id: metadata.id,
            version: 1,
            name: metadata.name.clone(),
            full_name: format!("{}:{}", metadata.namespace, metadata.name), // TODO: Check that this is correct
            namespace: metadata.namespace,
            owner: metadata.owner,
            owner_ref: metadata.owner_ref,
            image: self.image.clone(),
            env: self.env.clone(),
            volumes: self.volumes.clone(),
            local_volumes: None,
            accelerators: self.accelerators.clone(),
            cpu_request: self.resources.as_ref().and_then(|r| r.cpu_request.clone()),
            memory_request: self.resources.as_ref().and_then(|r| r.memory_request.clone()),
            status: self.status.clone(),
            platform: Some(self.platform.clone()),
            platforms: None,
            resource_name: None,
            resource_namespace: None,
            resource_cost_per_hr: None,
            command: self.command.clone(),
            args: self.args.clone(),
            labels: Some(metadata.labels),
            meters: self.meters.clone(),
            queue: self.queue.clone(),
            ports: self.ports.clone(),
            proxy_port: self.proxy_port,
            timeout: self.timeout.clone(),
            resources: Some(self.resources),
            health_check: Some(self.health_check),
            restart: self.restart.clone(),
            authz: Some(self.authz),
            public_addr: None,
            tailnet_ip: None,
            created_by: Some(metadata.created_by),
            desired_status: None,
            controller_data: None,
            container_user: None,
            ssh_keys: Some(self.ssh_keys),
            updated_at: chrono::Utc::now().timestamp(),
            created_at: chrono::Utc::now().timestamp(),
        };

        Ok(model)
    }
}