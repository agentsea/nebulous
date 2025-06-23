// src/query.rs
use crate::entities::containers;
use crate::entities::namespaces;
use crate::entities::processors;
use crate::entities::secrets;
use crate::resources::v1::containers::base::ContainerStatus;
use crate::resources::v1::containers::models::V1ContainerStatus;
use sea_orm::sea_query::Expr;
use sea_orm::Value;
use sea_orm::*;
use sea_orm::{ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter};

pub struct Query;

impl Query {
    pub async fn find_containers_by_owners(
        db: &DatabaseConnection,
        owners: &[&str],
    ) -> Result<Vec<containers::Model>, DbErr> {
        containers::Entity::find()
            .filter(containers::Column::Owner.is_in(owners.iter().copied()))
            .all(db)
            .await
    }
    pub async fn find_container_by_id(
        db: &DatabaseConnection,
        id: String,
    ) -> Result<Option<containers::Model>, DbErr> {
        containers::Entity::find_by_id(id).one(db).await
    }

    pub async fn find_container_by_namespace_and_name(
        db: &DatabaseConnection,
        namespace: &str,
        name: &str,
    ) -> Result<Option<containers::Model>, DbErr> {
        containers::Entity::find()
            .filter(containers::Column::Namespace.eq(namespace))
            .filter(containers::Column::Name.eq(name))
            .one(db)
            .await
    }

    pub async fn find_container_by_namespace_name_and_owners(
        db: &DatabaseConnection,
        namespace: &str,
        name: &str,
        owners: &[&str],
    ) -> Result<containers::Model, DbErr> {
        let result = containers::Entity::find()
            .filter(containers::Column::Namespace.eq(namespace))
            .filter(containers::Column::Name.eq(name))
            .filter(containers::Column::Owner.is_in(owners.iter().copied()))
            .one(db)
            .await?;

        result.ok_or(DbErr::RecordNotFound(format!(
            "Container with namespace '{namespace}' and name '{name}' not found for the specified owners"
        )))
    }

    pub async fn find_container_by_id_and_owners(
        db: &DatabaseConnection,
        id: &str,
        owners: &[&str],
    ) -> Result<containers::Model, DbErr> {
        let result = containers::Entity::find()
            .filter(containers::Column::Id.eq(id))
            .filter(containers::Column::Owner.is_in(owners.iter().copied()))
            .one(db)
            .await?;

        result.ok_or(DbErr::RecordNotFound(format!(
            "Container with id '{}' not found for the specified owners",
            id
        )))
    }

    /// Fetches an agent key from the secrets table by container ID
    pub async fn get_agent_key(
        db: &DatabaseConnection,
        container_id: String,
    ) -> Result<Option<String>, DbErr> {
        use crate::entities::secrets;

        // Look for a secret with the same ID as the container
        let secret = secrets::Entity::find_by_id(container_id).one(db).await?;

        // If found, decrypt the value and return it
        if let Some(secret) = secret {
            match secret.decrypt_value() {
                Ok(value) => Ok(Some(value)),
                Err(e) => Err(DbErr::Custom(format!("Failed to decrypt agent key: {}", e))),
            }
        } else {
            // No secret found for this container ID
            Ok(None)
        }
    }

    /// Fetches all containers from the database
    pub async fn find_all_containers(
        db: &DatabaseConnection,
    ) -> Result<Vec<containers::Model>, DbErr> {
        containers::Entity::find().all(db).await
    }

    /// Fetches the status of a container by its ID
    pub async fn get_container_status(
        db: &DatabaseConnection,
        container_id: &str,
    ) -> Result<Option<V1ContainerStatus>, DbErr> {
        let container = containers::Entity::find_by_id(container_id)
            .select_only()
            .column(containers::Column::Status)
            .one(db)
            .await?;

        // Deserialize the status string into V1ContainerStatus if container exists
        Ok(container.and_then(|c| {
            c.status
                .map(|json_value| serde_json::from_value::<V1ContainerStatus>(json_value))
                .transpose()
                .ok()
                .flatten()
        }))
    }

    /// Fetches all active containers from the database by inspecting the "status" key in the status JSON.
    pub async fn find_all_active_containers(
        db: &DatabaseConnection,
    ) -> Result<Vec<containers::Model>, DbErr> {
        use crate::resources::v1::containers::base::ContainerStatus;
        use sea_orm::{Condition, Value};

        // Convert your set of active statuses to lowercase strings
        let active_statuses = vec![
            ContainerStatus::Defined.to_string().to_lowercase(),
            ContainerStatus::Creating.to_string().to_lowercase(),
            ContainerStatus::Paused.to_string().to_lowercase(),
            ContainerStatus::Queued.to_string().to_lowercase(),
            ContainerStatus::Running.to_string().to_lowercase(),
            ContainerStatus::Pending.to_string().to_lowercase(),
            ContainerStatus::Restarting.to_string().to_lowercase(),
            ContainerStatus::Created.to_string().to_lowercase(),
        ];

        // Build a condition for each status and combine them with OR
        let mut status_condition = Condition::any();
        for status in active_statuses {
            // Use lower() in SQL for case-insensitive comparison
            status_condition = status_condition.add(Expr::cust_with_values(
                "lower(status->>'status') = $1",
                [Value::from(status)],
            ));
        }

        containers::Entity::find()
            .filter(status_condition)
            .all(db)
            .await
    }

    /// Fetches all containers with a specific status (case-insensitive)
    pub async fn find_containers_by_status(
        db: &DatabaseConnection,
        status: crate::resources::v1::containers::base::ContainerStatus,
    ) -> Result<Vec<containers::Model>, DbErr> {
        let lowercase_status = status.to_string().to_lowercase();
        containers::Entity::find()
            .filter(Expr::cust_with_values(
                "lower(status->>'status') = $1",
                [Value::from(lowercase_status)],
            ))
            .all(db)
            .await
    }

    /// Return true if no other containers in the same queue
    /// are in an active or running state. Otherwise false.
    pub async fn is_queue_free(
        db: &DatabaseConnection,
        queue_name: &str,
        this_container_id: &str,
    ) -> Result<bool, DbErr> {
        // Define which statuses qualify as "active/running"
        // i.e., statuses that imply the container is still going or starting
        let active_like_statuses = vec![
            ContainerStatus::Defined.to_string().to_lowercase(),
            ContainerStatus::Creating.to_string().to_lowercase(),
            ContainerStatus::Created.to_string().to_lowercase(),
            ContainerStatus::Queued.to_string().to_lowercase(),
            ContainerStatus::Pending.to_string().to_lowercase(),
            ContainerStatus::Running.to_string().to_lowercase(),
            ContainerStatus::Restarting.to_string().to_lowercase(),
        ];

        // Build a condition that checks if lower(status->>'status') is in one of these
        let mut active_condition = Condition::any();
        for status_str in active_like_statuses {
            active_condition = active_condition.add(Expr::cust_with_values(
                "lower(status->>'status') = $1",
                [Value::from(status_str)],
            ));
        }

        // First check if there's any active container in the queue
        let another_active_container = containers::Entity::find()
            .filter(containers::Column::Queue.eq(queue_name))
            .filter(containers::Column::Id.ne(this_container_id))
            .filter(active_condition)
            .one(db)
            .await?;

        if another_active_container.is_some() {
            return Ok(false);
        }

        // If no active containers, check if this is the next container in line
        // Compare against lowercase 'queued'
        let queued_status_lower = ContainerStatus::Queued.to_string().to_lowercase();
        let next_container = containers::Entity::find()
            .filter(containers::Column::Queue.eq(queue_name))
            .filter(containers::Column::Id.ne(this_container_id))
            .filter(Expr::cust_with_values(
                "lower(status->>'status') = $1",
                [Value::from(queued_status_lower)],
            ))
            .order_by_asc(containers::Column::CreatedAt)
            .one(db)
            .await?;

        // If there's a queued container with an earlier creation time, this container should wait
        if let Some(earlier_container) = next_container {
            let this_container = containers::Entity::find_by_id(this_container_id)
                .one(db)
                .await?;

            if let Some(this_container) = this_container {
                if earlier_container.created_at < this_container.created_at {
                    return Ok(false);
                }
            }
        }

        Ok(true)
    }

    /// Fetch and decrypt `(private_key, public_key)` for a container by ID.
    /// Returns a tuple of `Option<String>` for (private_key, public_key).
    pub async fn get_ssh_keypair(
        db: &DatabaseConnection,
        container_id: &str,
    ) -> Result<(Option<String>, Option<String>), DbErr> {
        let private_secret_id = format!("ssh-private-key-{}", container_id);
        let public_secret_id = format!("ssh-public-key-{}", container_id);

        // Fetch both secrets at once
        let secrets_records = secrets::Entity::find()
            .filter(
                secrets::Column::Id
                    .is_in(vec![private_secret_id.clone(), public_secret_id.clone()]),
            )
            .all(db)
            .await?;

        let mut private_key: Option<String> = None;
        let mut public_key: Option<String> = None;

        // Decrypt each record, matching by ID
        for record in secrets_records {
            match record.id.as_str() {
                x if x == private_secret_id => {
                    private_key = Some(record.decrypt_value().map_err(|e| {
                        DbErr::Custom(format!(
                            "Failed to decrypt SSH private key for container {container_id}: {e}"
                        ))
                    })?);
                }
                x if x == public_secret_id => {
                    public_key = Some(record.decrypt_value().map_err(|e| {
                        DbErr::Custom(format!(
                            "Failed to decrypt SSH public key for container {container_id}: {e}"
                        ))
                    })?);
                }
                _ => {}
            }
        }

        match std::env::var("RUNPOD_PRIVATE_KEY") {
            Ok(runpod_private_key) => {
                private_key = Some(runpod_private_key);
            }
            Err(_) => {}
        }

        Ok((private_key, public_key))
    }

    /// Find a single secret by ID and ensure that the user is an owner
    pub async fn find_secret_by_id_and_owners(
        db: &DatabaseConnection,
        id: &str,
        owners: &[&str],
    ) -> Result<secrets::Model, DbErr> {
        let result = secrets::Entity::find()
            .filter(secrets::Column::Id.eq(id))
            .filter(secrets::Column::Owner.is_in(owners.iter().copied()))
            .one(db)
            .await?;

        result.ok_or(DbErr::RecordNotFound(format!(
            "Secret with id '{}' not found for the specified owners",
            id
        )))
    }

    pub async fn find_secret_by_namespace_and_name(
        db: &DatabaseConnection,
        namespace: &str,
        name: &str,
    ) -> Result<Option<secrets::Model>, DbErr> {
        secrets::Entity::find()
            .filter(secrets::Column::Namespace.eq(namespace))
            .filter(secrets::Column::Name.eq(name))
            .one(db)
            .await
    }

    /// Fetch all secrets for a given list of owners
    pub async fn find_secrets_by_owners(
        db: &DatabaseConnection,
        owners: &[&str],
    ) -> Result<Vec<secrets::Model>, DbErr> {
        secrets::Entity::find()
            .filter(secrets::Column::Owner.is_in(owners.iter().copied()))
            .all(db)
            .await
    }

    /// Fetch all processors for a given list of owners
    pub async fn find_processors_by_owners(
        db: &DatabaseConnection,
        owners: &[&str],
    ) -> Result<Vec<processors::Model>, DbErr> {
        processors::Entity::find()
            .filter(processors::Column::Owner.is_in(owners.iter().copied()))
            .all(db)
            .await
    }

    /// Finds a processor by namespace, name and owners
    pub async fn find_processor_by_namespace_name_and_owners(
        db: &DatabaseConnection,
        namespace: &str,
        name: &str,
        owners: &[&str],
    ) -> Result<processors::Model, DbErr> {
        let result = processors::Entity::find()
            .filter(processors::Column::Namespace.eq(namespace))
            .filter(processors::Column::Name.eq(name))
            .filter(processors::Column::Owner.is_in(owners.iter().copied()))
            .one(db)
            .await?;

        result.ok_or(DbErr::RecordNotFound(format!(
            "Processor with namespace '{namespace}' and name '{name}' not found for the specified owners"
        )))
    }

    /// Finds a processor by ID and owners
    pub async fn find_processor_by_id_and_owners(
        db: &DatabaseConnection,
        id: &str,
        owners: &[&str],
    ) -> Result<processors::Model, DbErr> {
        let result = processors::Entity::find()
            .filter(processors::Column::Id.eq(id))
            .filter(processors::Column::Owner.is_in(owners.iter().copied()))
            .one(db)
            .await?;

        result.ok_or(DbErr::RecordNotFound(format!(
            "Processor with id '{}' not found for the specified owners",
            id
        )))
    }

    /// Fetches all active processors from the database by inspecting the "status" key in the status JSON.
    pub async fn find_all_active_processors(
        db: &DatabaseConnection,
    ) -> Result<Vec<processors::Model>, DbErr> {
        use crate::resources::v1::processors::base::ProcessorStatus;
        use sea_orm::{Condition, Value};

        // Convert your set of active statuses to lowercase strings
        let active_statuses = vec![
            ProcessorStatus::Defined.to_string().to_lowercase(),
            ProcessorStatus::Creating.to_string().to_lowercase(),
            ProcessorStatus::Created.to_string().to_lowercase(),
            ProcessorStatus::Pending.to_string().to_lowercase(),
            ProcessorStatus::Running.to_string().to_lowercase(),
            ProcessorStatus::Scaling.to_string().to_lowercase(),
        ];

        // Build a condition for each status and combine them with OR
        let mut status_condition = Condition::any();
        for status in active_statuses {
            // Use lower() in SQL for case-insensitive comparison
            status_condition = status_condition.add(Expr::cust_with_values(
                "lower(status->>'status') = $1",
                [Value::from(status)],
            ));
        }

        processors::Entity::find()
            .filter(status_condition)
            .all(db)
            .await
    }

    /// Finds containers whose JSON `metadata` field has `"owner_ref"` matching `owner_ref_value`.
    pub async fn find_containers_by_owner_ref(
        db: &DatabaseConnection,
        owner_ref_value: &str,
    ) -> Result<Vec<containers::Model>, DbErr> {
        containers::Entity::find()
            .filter(containers::Column::OwnerRef.eq(owner_ref_value))
            .all(db)
            .await
    }

    /// Finds containers by their resource_name (e.g., EC2 instance ID).
    pub async fn find_containers_by_resource_name(
        db: &DatabaseConnection,
        resource_name: &str,
    ) -> Result<Vec<containers::Model>, DbErr> {
        containers::Entity::find()
            .filter(containers::Column::ResourceName.eq(resource_name))
            .all(db)
            .await
    }

    /// Find a volume by namespace, name, and owners
    pub async fn find_volume_by_namespace_name_and_owners(
        db: &DatabaseConnection,
        namespace: &str,
        name: &str,
        owners: &[&str],
    ) -> Result<crate::entities::volumes::Model, DbErr> {
        use crate::entities::volumes;

        let result = volumes::Entity::find()
            .filter(volumes::Column::Namespace.eq(namespace))
            .filter(volumes::Column::Name.eq(name))
            .filter(volumes::Column::Owner.is_in(owners.iter().copied()))
            .one(db)
            .await?;

        result.ok_or(DbErr::RecordNotFound(format!(
            "Volume with namespace '{namespace}' and name '{name}' not found for the specified owners"
        )))
    }

    /// Counts the number of active containers for a processor
    pub async fn count_active_containers_for_processor(
        db: &DatabaseConnection,
        processor_id: &str,
    ) -> Result<u64, DbErr> {
        use crate::resources::v1::containers::base::ContainerStatus;
        use sea_orm::{Condition, Value};

        // Retrieve the processor to build the owner_ref format
        let processor = processors::Entity::find_by_id(processor_id)
            .one(db)
            .await?
            .ok_or(DbErr::RecordNotFound(format!(
                "Processor with id {} not found",
                processor_id
            )))?;

        // Build the owner_ref in the format "name.namespace.Processor"
        let owner_ref = format!("{}.{}.Processor", processor.name, processor.namespace);

        // Define active statuses (lowercase)
        let active_statuses = vec![
            ContainerStatus::Defined.to_string().to_lowercase(),
            ContainerStatus::Creating.to_string().to_lowercase(),
            ContainerStatus::Created.to_string().to_lowercase(),
            ContainerStatus::Queued.to_string().to_lowercase(),
            ContainerStatus::Pending.to_string().to_lowercase(),
            ContainerStatus::Running.to_string().to_lowercase(),
            ContainerStatus::Restarting.to_string().to_lowercase(),
            ContainerStatus::Paused.to_string().to_lowercase(),
        ];

        // Build status condition using lower() in SQL
        let mut status_condition = Condition::any();
        for status in active_statuses {
            status_condition = status_condition.add(Expr::cust_with_values(
                "lower(status->>'status') = $1",
                [Value::from(status)],
            ));
        }

        // Find containers with this processor as owner_ref and with active status
        let count = containers::Entity::find()
            .filter(containers::Column::OwnerRef.eq(owner_ref))
            .filter(status_condition)
            .count(db)
            .await?;

        Ok(count)
    }

    /// Fetch all namespaces for a given list of owners
    pub async fn find_namespaces_by_owners(
        db: &DatabaseConnection,
        owners: &[&str],
    ) -> Result<Vec<namespaces::Model>, DbErr> {
        namespaces::Entity::find()
            .filter(namespaces::Column::Owner.is_in(owners.iter().copied()))
            .all(db)
            .await
    }
}
