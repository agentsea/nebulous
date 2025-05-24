use anyhow::Result;
use aws_config::{self, BehaviorVersion, Region, SdkConfig};
use aws_sdk_iam::Client as IamClient;
use aws_sdk_s3::config::{Credentials, Region as S3Region};
use aws_sdk_s3::Client as S3Client;
use aws_sdk_sts::primitives::DateTime;
use aws_sdk_sts::Client as StsClient;
use serde_json::{json, to_string};
use std::env;
use tracing::{debug, error, info, warn};

pub struct S3ClientInternal {
    client: S3Client,
    bucket: String,
    base_path: String,
}

impl S3ClientInternal {
    pub fn new(
        access_key: &str,
        secret_key: &str,
        bucket: &str,
        namespace: &str,
        name: &str,
    ) -> Result<Self> {
        let credentials = Credentials::new(
            access_key,
            secret_key,
            None, // No session token needed for permanent credentials
            None, // No expiration
            "permanent-credentials",
        );

        let config = aws_sdk_s3::Config::builder()
            .region(Region::new("us-east-1")) // adjust as needed
            .credentials_provider(credentials)
            .build();

        let client = S3Client::from_conf(config);
        let base_path = format!("data/{}/{}", namespace, name);

        Ok(Self {
            client,
            bucket: bucket.to_string(),
            base_path,
        })
    }

    // Helper method to construct full path
    fn full_path(&self, key: &str) -> String {
        format!("{}/{}", self.base_path, key.trim_matches('/'))
    }

    // Example methods for S3 operations
    pub async fn put_object(&self, key: &str, data: Vec<u8>) -> Result<()> {
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(self.full_path(key))
            .body(data.into())
            .send()
            .await?;
        Ok(())
    }

    pub async fn get_object(&self, key: &str) -> Result<Vec<u8>> {
        let response = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(self.full_path(key))
            .send()
            .await?;

        Ok(response.body.collect().await?.into_bytes().to_vec())
    }

    pub async fn list_objects(&self, prefix: Option<&str>) -> Result<Vec<String>> {
        let prefix = match prefix {
            Some(p) => format!("{}/{}", self.base_path, p.trim_matches('/')),
            None => self.base_path.clone(),
        };

        let response = self
            .client
            .list_objects_v2()
            .bucket(&self.bucket)
            .prefix(prefix)
            .send()
            .await?;

        let keys: Vec<String> = response
            .contents()
            .iter()
            .filter_map(|obj| obj.key.clone())
            .collect();

        Ok(keys)
    }
}

pub struct IamCredentials {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub username: String,
}

pub struct StsCredentials {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: String,
    pub expiration: Option<DateTime>,
}

pub async fn create_s3_scoped_user(
    bucket_name: &str,
    namespace: &str,
    name: &str,
) -> Result<IamCredentials> {
    let config = aws_config::defaults(BehaviorVersion::latest())
        .region(Region::new("us-east-1"))
        .load()
        .await;
    let client = IamClient::new(&config);

    // Create a unique username
    let username = format!("nebu-{}-{}", namespace, name);

    // Create the IAM user
    match env::var("AWS_NEBU_USER_BOUNDARY") {
        Ok(boundary_arn) => {
            client.create_user()
                .user_name(&username)
                .permissions_boundary(&boundary_arn)
                .send()
                .await?;
        }
        Err(_) => {
            client.create_user()
                .user_name(&username)
                .send()
                .await?;
        }
    }

    let policy_document = json!({
      "Version": "2012-10-17",
      "Statement": [
        // -- 1) Allow listing objects only under data/<namespace> prefix
        {
          "Effect": "Allow",
          "Action": "s3:ListBucket",
          "Resource": [
            format!("arn:aws:s3:::{}", bucket_name)
          ],
          "Condition": {
            "StringLike": {
              "s3:prefix": [
                format!("data/{}/", namespace),
                format!("data/{}/*", namespace)
              ]
            }
          }
        },
        // -- 2) Allow working with objects under data/<namespace> prefix
        {
          "Effect": "Allow",
          "Action": [
            "s3:*"
          ],
          "Resource": [
            format!("arn:aws:s3:::{}/data/{}", bucket_name, namespace),
            format!("arn:aws:s3:::{}/data/{}/*", bucket_name, namespace)
          ]
        }
      ]
    });

    debug!(">>> Policy document: {}", policy_document);

    // Create the policy
    let policy_name = format!("s3-scope-{}-{}", namespace, name);
    client.put_user_policy()
        .user_name(&username)
        .policy_name(&policy_name)
        .policy_document(to_string(&policy_document)?)
        .send()
        .await?;

    // Create access key for the user
    let key_response = client
        .create_access_key()
        .user_name(&username)
        .send()
        .await?;

    let access_key = key_response.access_key().unwrap();

    Ok(IamCredentials {
        access_key_id: access_key.access_key_id().to_string(),
        secret_access_key: access_key.secret_access_key().to_string(),
        username,
    })
}

pub async fn delete_s3_scoped_user(namespace: &str, name: &str) -> Result<()> {
    let config = aws_config::defaults(BehaviorVersion::latest())
        .region(Region::new("us-east-1"))
        .load()
        .await;
    let client = IamClient::new(&config);

    let username = format!("nebu-{}-{}", namespace, name);

    // --- 1. Delete Access Keys ---
    debug!("Attempting to delete access keys for user: {}", username);
    match client.list_access_keys().user_name(&username).send().await {
        Ok(keys_output) => {
            for key_metadata in keys_output.access_key_metadata() {
                if let Some(key_id) = key_metadata.access_key_id() {
                    debug!("Deleting access key {} for user {}", key_id, username);
                    match client
                        .delete_access_key()
                        .user_name(&username)
                        .access_key_id(key_id)
                        .send()
                        .await
                    {
                        Ok(_) => debug!("Successfully deleted access key {}", key_id),
                        Err(e) => {
                            // Check if it's a NoSuchEntity error (key already gone)
                            if let Some(aws_err) = e.as_service_error() {
                                if aws_err.is_no_such_entity_exception() {
                                    warn!("Access key {} not found for user {}, likely already deleted.", key_id, username);
                                } else {
                                    error!("Failed to delete access key {}: {}", key_id, e);
                                    // Decide if this should be fatal. Continuing for now.
                                }
                            } else {
                                error!("Failed to delete access key {}: {}", key_id, e);
                            }
                        }
                    }
                } else {
                    warn!(
                        "Found access key metadata without an ID for user {}",
                        username
                    );
                }
            }
        }
        Err(e) => {
            // Check if the user doesn't exist
            if let Some(aws_err) = e.as_service_error() {
                if aws_err.is_no_such_entity_exception() {
                    warn!(
                        "User {} not found when listing access keys, assuming already deleted.",
                        username
                    );
                    // If user doesn't exist, we can potentially stop here or attempt policy cleanup if ARN is known/constructible
                    return Ok(()); // Assuming successful deletion if user gone
                } else {
                    error!("Failed to list access keys for user {}: {}", username, e);
                    return Err(e.into());
                }
            } else {
                error!("Failed to list access keys for user {}: {}", username, e);
                return Err(e.into());
            }
        }
    }

    // --- 2. Delete the User ---
    debug!("Attempting to delete user: {}", username);
    match client.delete_user().user_name(&username).send().await {
        Ok(_) => debug!("Successfully deleted user {}", username),
        Err(e) => {
            if let Some(aws_err) = e.as_service_error() {
                if aws_err.is_no_such_entity_exception() {
                    warn!(
                        "User {} not found during deletion, assuming already deleted.",
                        username
                    );
                } else if aws_err.is_delete_conflict_exception() {
                    error!("Failed to delete user {} due to conflict (resources might still be attached): {}", username, e);
                    // This indicates a problem with the previous cleanup steps. Return error.
                    return Err(e.into());
                } else {
                    error!("Failed to delete user {}: {}", username, e);
                    return Err(e.into());
                }
            } else {
                error!("Failed to delete user {}: {}", username, e);
                return Err(e.into());
            }
        }
    }

    info!(
        "Successfully deleted S3 scoped user and associated resources for {}",
        username
    );
    Ok(())
}

/// Generate temporary AWS credentials with a specific S3 path restriction using federation tokens.
/// This approach uses STS GetFederationToken with an inline policy for proper restrictions.
pub async fn generate_temporary_s3_credentials(
    bucket_name: &str,
    namespace: &str,
    duration_seconds: i32,
) -> Result<StsCredentials> {
    let config = aws_config::defaults(BehaviorVersion::latest())
        .region(Region::new("us-east-1"))
        .load()
        .await;
    let sts_client = StsClient::new(&config);

    // Create a friendly name for the federated session
    let federated_user_name = format!("nebulous-{}-session", namespace);
    // Ensure name meets requirements (alphanumeric + [,_.=@-], length <=32)
    let federated_user_name: String = federated_user_name
        .chars()
        .filter(|c| c.is_alphanumeric() || [',', '.', '_', '=', '@', '-'].contains(c))
        .take(32)
        .collect();

    // Define the inline policy document restricting access to namespace path
    let policy_document = json!({
        "Version": "2012-10-17",
        "Statement": [
            {
                "Effect": "Allow",
                "Action": [
                    "s3:ListBucket"
                ],
                "Resource": [
                    format!("arn:aws:s3:::{}", bucket_name)
                ],
                "Condition": {
                    "StringLike": {
                        "s3:prefix": [
                            format!("data/{}/", namespace),
                            format!("data/{}/*", namespace)
                        ]
                    }
                }
            },
            {
                "Effect": "Allow",
                "Action": [
                    "s3:GetObject",
                    "s3:PutObject",
                    "s3:DeleteObject"
                ],
                "Resource": [
                    format!("arn:aws:s3:::{}/data/{}/*", bucket_name, namespace)
                ]
            }
        ]
    });

    let policy_string = policy_document.to_string();
    debug!("Federation Token Policy: {}", policy_string);

    // Request federation token with policy restrictions
    debug!("Requesting federation token for: {}", federated_user_name);

    let federation_token_output = sts_client
        .get_federation_token()
        .name(&federated_user_name)
        .policy(policy_string)
        .duration_seconds(duration_seconds)
        .send()
        .await?;

    match federation_token_output.credentials() {
        Some(creds) => {
            info!("Successfully obtained policy-restricted federation token credentials for namespace {}", namespace);

            if let Some(federated_user) = federation_token_output.federated_user() {
                debug!("Federated User ARN: {}", federated_user.arn());
            }

            Ok(StsCredentials {
                access_key_id: creds.access_key_id().to_string(),
                secret_access_key: creds.secret_access_key().to_string(),
                session_token: creds.session_token().to_string(),
                expiration: Some(creds.expiration().clone()),
            })
        }
        None => {
            error!("GetFederationToken succeeded but returned no credentials");
            Err(anyhow::anyhow!(
                "GetFederationToken returned no credentials"
            ))
        }
    }
}
