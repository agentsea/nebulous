use crate::entities::containers;
use crate::entities::processors;
use crate::entities::secrets;
use crate::models::V1ProcessorStatus;
use crate::models::{V1ContainerStatus, V1UpdateContainer};
use sea_orm::prelude::Json;
use sea_orm::*;
use serde_json::json;
use tracing::info;

pub struct Mutation;

impl Mutation {
    pub async fn create_container(
        db: &DatabaseConnection,
        form_data: containers::ActiveModel,
    ) -> Result<containers::Model, DbErr> {
        form_data.insert(db).await
    }

    /// Mutation to update the resource_name field in a container
    pub async fn update_container_resource_name(
        db: &DatabaseConnection,
        id: String,
        resource_name: String,
    ) -> Result<containers::Model, DbErr> {
        let container = containers::Entity::find_by_id(id)
            .one(db)
            .await?
            .ok_or(DbErr::Custom("Container not found".to_string()))?;

        let mut container: containers::ActiveModel = container.into();

        container.resource_name = Set(Some(resource_name));
        container.updated_at = Set(chrono::Utc::now().into());

        container.update(db).await
    }

    /// Mutation to update the resource_cost_per_hr field in a container
    pub async fn update_container_resource_cost_per_hr(
        db: &DatabaseConnection,
        id: String,
        resource_cost_per_hr: f64,
    ) -> Result<containers::Model, DbErr> {
        let container = containers::Entity::find_by_id(id)
            .one(db)
            .await?
            .ok_or(DbErr::Custom("Container not found".to_string()))?;

        let mut container: containers::ActiveModel = container.into();

        container.resource_cost_per_hr = Set(Some(resource_cost_per_hr));
        container.updated_at = Set(chrono::Utc::now().into());

        container.update(db).await
    }

    /// Mutation to update the container "pod IP"
    pub async fn update_container_pod_ip(
        db: &DatabaseConnection,
        id: String,
        pod_ip: Option<String>,
    ) -> Result<containers::Model, DbErr> {
        let container = containers::Entity::find_by_id(id)
            .one(db)
            .await?
            .ok_or(DbErr::Custom("Container not found".to_string()))?;

        let mut container: containers::ActiveModel = container.into();

        container.public_addr = Set(pod_ip);
        container.updated_at = Set(chrono::Utc::now().into());

        container.update(db).await
    }

    // Mutation to update only the container status
    pub async fn update_container_status(
        db: &DatabaseConnection,
        id: String,
        status: Option<String>,
        message: Option<String>,
        accelerator: Option<String>,
        public_ip: Option<String>,
    ) -> Result<containers::Model, DbErr> {
        let container = containers::Entity::find_by_id(id)
            .one(db)
            .await?
            .ok_or(DbErr::Custom("Container not found".to_string()))?;

        let mut container: containers::ActiveModel = container.into();

        // 1. Parse any existing status from the database
        let mut existing_status = match &container.status {
            ActiveValue::Set(Some(val)) => {
                serde_json::from_value::<V1ContainerStatus>(val.clone()).unwrap_or_default()
            }
            _ => V1ContainerStatus::default(),
        };

        info!(
            "[Mutation] Updating container status to {:?}",
            existing_status
        );

        // 2. Merge in only the new fields
        if let Some(s) = status {
            existing_status.status = Some(s);
        }
        if let Some(m) = message {
            existing_status.message = Some(m);
        }
        if let Some(a) = accelerator {
            existing_status.accelerator = Some(a);
        }
        if let Some(ip) = public_ip {
            existing_status.public_ip = Some(ip);
        }

        // 3. Store the merged status back as JSON
        container.status = Set(Some(serde_json::json!(existing_status)));
        container.updated_at = Set(chrono::Utc::now().into());

        info!(
            "[Mutation] Updating container status to {:?}",
            container.status
        );

        // 4. Update in the database
        container.update(db).await
    }

    // Mutation to update multiple container fields
    pub async fn update_container(
        db: &DatabaseConnection,
        id: String,
        update_data: V1UpdateContainer,
    ) -> Result<containers::Model, DbErr> {
        let container = containers::Entity::find_by_id(id)
            .one(db)
            .await?
            .ok_or(DbErr::Custom("Container not found".to_string()))?;

        let mut container: containers::ActiveModel = container.into();

        if let Some(image) = update_data.image {
            container.image = Set(image);
        }

        if let Some(env) = update_data.env {
            container.env = Set(Some(json!(env).into()));
        }

        if let Some(command) = update_data.command {
            container.command = Set(Some(command));
        }

        if let Some(volumes) = update_data.volumes {
            container.volumes = Set(Some(json!(volumes).into()));
        }

        if let Some(accelerators) = update_data.accelerators {
            container.accelerators = Set(Some(accelerators));
        }

        if let Some(labels) = update_data.labels {
            container.labels = Set(Some(json!(labels).into()));
        }

        if let Some(cpu_request) = update_data.cpu_request {
            container.cpu_request = Set(Some(cpu_request));
        }

        if let Some(memory_request) = update_data.memory_request {
            container.memory_request = Set(Some(memory_request));
        }

        if let Some(platform) = update_data.platform {
            container.platform = Set(Some(platform));
        }

        // Always update the updated_at timestamp
        container.updated_at = Set(chrono::Utc::now().into());

        container.update(db).await
    }

    // Mutation to delete a container by ID
    pub async fn delete_container(
        db: &DatabaseConnection,
        id: String,
    ) -> Result<DeleteResult, DbErr> {
        let result = containers::Entity::delete_by_id(id).exec(db).await?;

        // Check if any row was actually deleted
        if result.rows_affected == 0 {
            return Err(DbErr::Custom("Container not found".to_string()));
        }

        Ok(result)
    }

    /// Store a container's SSH keypair (private & public) in the `secrets` table.
    /// Returns tuples (private_key_secret, public_key_secret).
    pub async fn store_ssh_keypair(
        db: &DatabaseConnection,
        container_id: &str,
        namespace: &str,
        private_key: &str,
        public_key: &str,
        owner_id: &str,
    ) -> Result<(secrets::Model, secrets::Model), Box<dyn std::error::Error + Send + Sync>> {
        // 1) Create unique IDs for the secrets (you can pick your own naming).
        let private_secret_id = format!("ssh-private-key-{}", container_id);
        let public_secret_id = format!("ssh-public-key-{}", container_id);

        // 2) Build the ActiveModels for each secret.
        //    The `secrets::Model::new()` will automatically encrypt the `value`.
        let private_secret_model = secrets::Model::new(
            private_secret_id.clone(),
            format!("SSH private key for container {}", container_id),
            namespace.to_string(),
            owner_id.to_string(),
            private_key,
            Some(owner_id.to_string()),
            None, // Labels optional
        )
        .map_err(|e| {
            format!(
                "Error creating new secret model for private key [{}]: {e}",
                private_secret_id
            )
        })?;

        let public_secret_model = secrets::Model::new(
            public_secret_id.clone(),
            format!("SSH public key for container {}", container_id),
            namespace.to_string(),
            owner_id.to_string(),
            public_key,
            Some(owner_id.to_string()),
            None, // Labels optional
        )
        .map_err(|e| {
            format!(
                "Error creating new secret model for public key [{}]: {e}",
                public_secret_id
            )
        })?;

        // 3) Insert each secret into the DB.
        // (If you need upsert behavior, you'd adjust accordingly.)
        let private_inserted = secrets::ActiveModel::from(private_secret_model)
            .insert(db)
            .await
            .map_err(|e| format!("Failed to store private key: {e}"))?;

        let public_secret_active_model = secrets::ActiveModel::from(public_secret_model);
        let public_inserted = public_secret_active_model
            .insert(db)
            .await
            .map_err(|e| format!("Failed to store SSH public key: {e}"))?;

        Ok((private_inserted, public_inserted))
    }

    /// Update an existing secret by re-encrypting if `new_value` is provided.
    pub async fn update_secret(
        db: &DatabaseConnection,
        secret: secrets::Model,
        new_name: Option<String>,
        new_value: Option<String>,
        new_labels: Option<serde_json::Value>,
    ) -> Result<secrets::Model, DbErr> {
        let mut active_model = secrets::ActiveModel::from(secret);

        // If a new name is provided
        if let Some(name) = new_name {
            active_model.name = Set(name);
        }

        // If a new value is provided, re-encrypt
        if let Some(value) = new_value {
            let (encrypted_value, nonce) =
                secrets::Model::encrypt_value(&value).map_err(|e| DbErr::Custom(e))?;
            active_model.encrypted_value = Set(encrypted_value);
            active_model.nonce = Set(nonce);
        }

        // If new labels are provided
        if let Some(lbls) = new_labels {
            active_model.labels = Set(Some(lbls.into()));
        }

        // Always update updated_at
        active_model.updated_at = Set(chrono::Utc::now().into());

        active_model.update(db).await
    }

    /// Delete a secret by ID
    pub async fn delete_secret(
        db: &DatabaseConnection,
        id: String,
    ) -> Result<sea_orm::DeleteResult, DbErr> {
        secrets::Entity::delete_by_id(id).exec(db).await
    }

    /// Mutation to update just the `status` (and optionally `message`) of a processor.
    pub async fn update_processor_status(
        db: &DatabaseConnection,
        id: String,
        new_status: Option<String>,
        new_message: Option<String>,
    ) -> Result<processors::Model, DbErr> {
        // 1) Find the processor record by ID
        let processor = processors::Entity::find_by_id(id.clone())
            .one(db)
            .await?
            .ok_or_else(|| DbErr::Custom(format!("Processor '{}' not found", id)))?;

        // 2) Convert the existing `Model` into an `ActiveModel` so we can update fields
        let mut processor_am: processors::ActiveModel = processor.into();

        // 3) Parse any existing status in the processor record
        let mut existing_status = match &processor_am.status {
            Set(Some(val)) => serde_json::from_value::<V1ProcessorStatus>(val.clone())
                .unwrap_or_else(|_| {
                    info!("[Mutation] Existing processor status JSON was invalid, defaulting.");
                    V1ProcessorStatus::default()
                }),
            _ => V1ProcessorStatus::default(),
        };

        info!(
            "[Mutation] Current processor status for '{}': {:?}",
            id, existing_status
        );

        // 4) Merge in the new optional fields if provided
        if let Some(s) = new_status {
            existing_status.status = Some(s);
        }
        if let Some(m) = new_message {
            existing_status.message = Some(m);
        }

        // 5) Update the ActiveModel to hold the new status as JSON
        processor_am.status = Set(Some(json!(existing_status)));
        // Always refresh the updated_at timestamp
        processor_am.updated_at = Set(chrono::Utc::now().into());

        info!(
            "[Mutation] Updating processor '{}' status to: {:?}",
            id, processor_am.status
        );

        // 6) Write it back to the database
        processor_am.update(db).await
    }
}
