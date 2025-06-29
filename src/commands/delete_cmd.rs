use crate::commands::request::server_request;
use futures::future::join_all;
use nebulous::config::ClientConfig;
use nebulous::resources::v1::containers::models::V1Containers;
use reqwest::Client;
use std::error::Error;

async fn list_containers_in_namespace(namespace: &str) -> Result<Vec<String>, Box<dyn Error>> {
    let url = format!("/v1/containers?namespace={}", namespace);
    let response = server_request(url.as_str(), reqwest::Method::GET).await?;

    if !response.status().is_success() {
        let error_text = response.text().await?;
        return Err(format!("Failed to list containers: {}", error_text).into());
    }

    let container_list: V1Containers = response.json().await?;
    let container_names = container_list
        .containers
        .into_iter()
        .map(|c| c.metadata.name)
        .collect();
    Ok(container_names)
}

pub async fn delete_container(
    name: Option<String>,
    namespace: Option<String>,
    delete_all: bool,
) -> Result<(), Box<dyn Error>> {
    let namespace = namespace.unwrap_or("-".to_string());
    let ns_trim = namespace.trim();

    if delete_all {
        println!(
            "Attempting to delete all containers in namespace '{}'...",
            ns_trim
        );
        let container_names = list_containers_in_namespace(ns_trim).await?;

        if container_names.is_empty() {
            println!("No containers found in namespace '{}'.", ns_trim);
            return Ok(());
        }

        let mut delete_futures = vec![];
        for container_name in container_names {
            let url = format!("/v1/containers/{}/{}", ns_trim, container_name.trim());
            println!(
                "Scheduling deletion for container: {}/{}",
                ns_trim,
                container_name.trim()
            );
            delete_futures.push(async move {
                let response = server_request(url.as_str(), reqwest::Method::DELETE).await;
                (container_name, response)
            });
        }

        let results = join_all(delete_futures).await;
        let mut errors = vec![];

        for (container_name, result) in results {
            match result {
                Ok(response) => {
                    if response.status().is_success() {
                        println!(
                            "Container '{}/{}' successfully deleted",
                            ns_trim,
                            container_name.trim()
                        );
                    } else {
                        let error_text = response
                            .text()
                            .await
                            .unwrap_or_else(|e| format!("Error reading response: {}", e));
                        let error_msg = format!(
                            "Failed to delete container '{}/{}': {}",
                            ns_trim,
                            container_name.trim(),
                            error_text
                        );
                        eprintln!("{}", error_msg);
                        errors.push(error_msg);
                    }
                }
                Err(e) => {
                    let error_msg = format!(
                        "Error during delete request for '{}/{}': {}",
                        ns_trim,
                        container_name.trim(),
                        e
                    );
                    eprintln!("{}", error_msg);
                    errors.push(error_msg);
                }
            }
        }

        if errors.is_empty() {
            println!(
                "All containers in namespace '{}' deleted successfully.",
                ns_trim
            );
            Ok(())
        } else {
            Err(format!(
                "Failed to delete some containers:
{}",
                errors.join(
                    "
"
                )
            )
            .into())
        }
    } else {
        let name = name.ok_or("Container name is required when not using --all")?;
        let url = format!("/v1/containers/{}/{}", ns_trim, name.trim());
        let response = server_request(url.as_str(), reqwest::Method::DELETE).await?;

        if response.status().is_success() {
            println!(
                "Container '{}/{}' successfully deleted",
                ns_trim,
                name.trim()
            );
            Ok(())
        } else {
            let error_text = response.text().await?;
            Err(format!("Failed to delete container: {}", error_text).into())
        }
    }
}

pub async fn delete_processor(
    name: String,
    namespace: Option<String>,
) -> Result<(), Box<dyn Error>> {
    let client = Client::new();
    let config = ClientConfig::read()?;
    let current_server = config.get_current_server_config().unwrap();
    let server = current_server.server.as_ref().unwrap();
    let api_key = current_server.api_key.as_ref().unwrap();
    let bearer_token = format!("Bearer {}", api_key);

    let namespace = namespace.unwrap_or("-".to_string());

    let url = format!(
        "{}/v1/processors/{}/{}",
        server,
        namespace.trim(),
        name.trim()
    );

    let response = client
        .delete(&url)
        .header("Authorization", &bearer_token)
        .send()
        .await?;

    if response.status().is_success() {
        println!("Processor '{}/{}' successfully deleted", namespace, name);
        Ok(())
    } else {
        let error_text = response.text().await?;
        Err(format!("Failed to delete processor: {}", error_text).into())
    }
}
