use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::commands::request::server_request;
use nebulous::config::ClientConfig;
use nebulous::resources::v1::containers::models::{V1Container, V1Containers};
use serde_json::Value;
use std::error::Error;
use tracing::debug;

pub async fn get_containers(id: Option<String>) -> Result<(), Box<dyn Error>> {
    let containers: Vec<V1Container> = match id {
        Some(id) => {
            let url = format!("/v1/containers/{}", id);
            let response = server_request(url.as_str(), reqwest::Method::GET).await?;
            let container: V1Container = response.json().await?;

            // For a specific container, output YAML instead of a table
            // Convert to YAML output
            let mut buf = Vec::new();
            {
                let mut serializer = serde_yaml::Serializer::new(&mut buf);
                container.serialize(&mut serializer)?;
            }
            let yaml = String::from_utf8(buf)?;
            println!("{}", yaml);
            return Ok(());

            // This is unreachable but kept for clarity
            // vec![container]
        }
        None => {
            let response = server_request("/v1/containers", reqwest::Method::GET).await?;
            let container_list: V1Containers = response.json().await?;
            container_list.containers
        }
    };

    // Create a table to display the containers
    let mut table = prettytable::Table::new();

    // Add table headers
    table.add_row(prettytable::Row::new(vec![
        prettytable::Cell::new("ID"),
        prettytable::Cell::new("NAME"),
        prettytable::Cell::new("NAMESPACE"),
        prettytable::Cell::new("IMAGE"),
        prettytable::Cell::new("PLATFORM"),
        prettytable::Cell::new("RESTART"),
        prettytable::Cell::new("STATUS"),
        prettytable::Cell::new("ACCELERATOR"),
        prettytable::Cell::new("PRICE"),
        prettytable::Cell::new("TAILNET URL"),
        prettytable::Cell::new("UPTIME"),
    ]));

    // Process containers data
    for container in containers {
        let id = container.metadata.id;
        let name = container.metadata.name;
        let namespace = container.metadata.namespace;
        let image = container.image;
        let platform = container.platform;
        let restart = container.restart;

        let status = match container.status.clone() {
            Some(status) => status.status.unwrap_or("N/A".to_string()),
            None => "N/A".to_string(),
        };

        let accelerator = match container.status.clone() {
            Some(status) => status.accelerator.unwrap_or("N/A".to_string()),
            None => "N/A".to_string(),
        };

        let tailnet_url = match container.status.clone() {
            Some(status) => status.tailnet_url.unwrap_or("N/A".to_string()),
            None => "N/A".to_string(),
        };

        let price_str = match container.status.clone() {
            Some(status) => status
                .cost_per_hr
                .map(|cost| format!("{:.2}", cost))
                .unwrap_or("N/A".to_string()),
            None => "N/A".to_string(),
        };

        let uptime = {
            let dt = DateTime::<Utc>::from_timestamp(container.metadata.created_at, 0)
                .unwrap_or_default();
            let duration = Utc::now().signed_duration_since(dt);

            if duration.num_days().abs() > 0 {
                format!("{}d", duration.num_days())
            } else if duration.num_hours().abs() > 0 {
                format!("{}hr", duration.num_hours())
            } else if duration.num_minutes().abs() > 0 {
                format!("{}m", duration.num_minutes())
            } else {
                format!("{}s", duration.num_seconds())
            }
        };

        // Add row to table
        table.add_row(prettytable::Row::new(vec![
            prettytable::Cell::new(&id),
            prettytable::Cell::new(&name),
            prettytable::Cell::new(&namespace),
            prettytable::Cell::new(&image),
            prettytable::Cell::new(&platform),
            prettytable::Cell::new(&restart),
            prettytable::Cell::new(&status),
            prettytable::Cell::new(&accelerator),
            prettytable::Cell::new(&price_str),
            prettytable::Cell::new(&tailnet_url),
            prettytable::Cell::new(&uptime),
        ]));
    }

    // Set table format and print
    table.set_format(*prettytable::format::consts::FORMAT_CLEAN);
    table.printstd();

    Ok(())
}

pub async fn get_secrets(id: Option<String>) -> Result<(), Box<dyn Error>> {
    let secrets = match id {
        Some(id) => {
            let url = format!("/v1/secrets/{}", id);
            let response = server_request(url.as_str(), reqwest::Method::GET).await?;
            let secret: Value = response.json().await?;
            vec![secret]
        }
        None => {
            let response = server_request("/v1/secrets", reqwest::Method::GET).await?;
            let secrets: Value = response.json().await?;
            secrets.as_array().unwrap_or(&Vec::new()).to_vec()
        }
    };

    // Create a table to display secrets
    let mut table = prettytable::Table::new();

    // Add table headers
    table.add_row(prettytable::Row::new(vec![
        prettytable::Cell::new("ID"),
        prettytable::Cell::new("NAME"),
        prettytable::Cell::new("NAMESPACE"),
        prettytable::Cell::new("CREATED"),
        prettytable::Cell::new("UPDATED"),
    ]));

    // Process each secret in the array
    for secret in secrets {
        if let Value::Object(secret_obj) = secret {
            // Access fields inside metadata
            let metadata = secret_obj.get("metadata").and_then(Value::as_object);

            let id = metadata
                .and_then(|m| m.get("id"))
                .and_then(Value::as_str)
                .unwrap_or("N/A");

            let name = metadata
                .and_then(|m| m.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("N/A");

            let namespace = metadata
                .and_then(|m| m.get("namespace"))
                .and_then(Value::as_str)
                .unwrap_or("N/A");

            // Handle creation time
            let created = secret_obj
                .get("metadata")
                .and_then(Value::as_object)
                .and_then(|m| m.get("created_at"))
                .and_then(Value::as_i64)
                .map(|timestamp| {
                    DateTime::<Utc>::from_timestamp(timestamp, 0)
                        .unwrap_or_default()
                        .format("%Y-%m-%d %H:%M:%S")
                        .to_string()
                })
                .unwrap_or("N/A".to_string());

            // Handle update time
            let updated = secret_obj
                .get("metadata")
                .and_then(Value::as_object)
                .and_then(|m| m.get("updated_at"))
                .and_then(Value::as_i64)
                .map(|timestamp| {
                    DateTime::<Utc>::from_timestamp(timestamp, 0)
                        .unwrap_or_default()
                        .format("%Y-%m-%d %H:%M:%S")
                        .to_string()
                })
                .unwrap_or("N/A".to_string());

            // Finally, add the row
            table.add_row(prettytable::Row::new(vec![
                prettytable::Cell::new(id),
                prettytable::Cell::new(name),
                prettytable::Cell::new(namespace),
                prettytable::Cell::new(&created),
                prettytable::Cell::new(&updated),
            ]));
        }
    }

    // Set table format and print
    table.set_format(*prettytable::format::consts::FORMAT_CLEAN);
    table.printstd();

    Ok(())
}

pub async fn get_accelerators(platform: Option<String>) -> Result<(), Box<dyn Error>> {
    use nebulous::accelerator::aws::AwsProvider;
    use nebulous::accelerator::base::{AcceleratorProvider, Config};
    use nebulous::accelerator::runpod::RunPodProvider;
    use prettytable::{format, Cell, Row, Table};

    // Load the default accelerator configuration
    let config = Config::default();

    // Create a table to display the accelerators
    let mut table = Table::new();

    // Add table headers
    let mut headers = vec![Cell::new("NAME"), Cell::new("MEMORY (GB)")];

    // If a platform is specified, add a column for platform-specific names
    if platform.is_some() {
        headers.push(Cell::new("PLATFORM NAME"));
    }

    table.add_row(Row::new(headers));

    // Get the appropriate provider based on the platform parameter
    let provider: Option<Box<dyn AcceleratorProvider>> = match platform.as_deref() {
        Some("aws") => Some(Box::new(AwsProvider::new())),
        Some("runpod") => Some(Box::new(RunPodProvider::new())),
        Some(unknown) => {
            eprintln!(
                "Unknown platform: {}. Supported platforms are 'aws' and 'runpod'.",
                unknown
            );
            return Ok(());
        }
        None => None,
    };

    // Add rows for each accelerator
    for acc in &config.accelerators.supported {
        let mut row = vec![Cell::new(&acc.name), Cell::new(&acc.memory.to_string())];

        // If a platform is specified, add the platform-specific name
        if let Some(ref provider) = provider {
            let platform_name = provider
                .get_platform_name(&acc.name)
                .map(|s| s.as_str())
                .unwrap_or("N/A");
            row.push(Cell::new(platform_name));
        }

        table.add_row(Row::new(row));
    }

    table.set_format(*format::consts::FORMAT_CLEAN);
    table.printstd();

    Ok(())
}

pub async fn get_processors(
    name: Option<String>,
    namespace: Option<String>,
) -> Result<(), Box<dyn Error>> {
    let config = ClientConfig::read()?;
    debug!("Config: {:?}", config);
    let current_server = config.get_current_server_config().unwrap();
    let server = current_server.server.as_ref().unwrap();
    let api_key = current_server.api_key.as_ref().unwrap();

    let bearer_token = format!("Bearer {}", api_key);
    let client = reqwest::Client::new();

    // If name and namespace are provided, fetch a single processor
    if let (Some(proc_name), Some(proc_namespace)) = (name, namespace) {
        let url = format!("{}/v1/processors/{}/{}", server, proc_namespace, proc_name);

        // Build the request
        let request = client.get(&url).header("Authorization", &bearer_token);

        // Execute the request
        let response = request.send().await?;

        // Check if the request was successful
        if !response.status().is_success() {
            return Err(format!("Failed to get processor: {}", response.status()).into());
        }

        // Parse the response
        let mut processor: Value = response.json().await?;

        // Remove null values for cleaner output
        remove_null_values(&mut processor);

        // Convert to YAML output
        let mut buf = Vec::new();
        {
            let mut serializer = serde_yaml::Serializer::new(&mut buf);
            processor.serialize(&mut serializer)?;
        }
        let yaml = String::from_utf8(buf)?;
        println!("{}", yaml);
        return Ok(());
    }

    // Otherwise, list all processors
    let url = format!("{}/v1/processors", server);

    // Build the request
    let request = client.get(&url).header("Authorization", &bearer_token);

    // Execute the request
    let response = request.send().await?;

    // Check if the request was successful
    if !response.status().is_success() {
        return Err(format!("Failed to get processors: {}", response.status()).into());
    }

    // Parse the response
    let mut processors_response: Value = response.json().await?;

    // Remove null values for cleaner output
    remove_null_values(&mut processors_response);

    // Create a table to display the processors
    let mut table = prettytable::Table::new();

    // Add table headers
    table.add_row(prettytable::Row::new(vec![
        prettytable::Cell::new("ID"),
        prettytable::Cell::new("NAME"),
        prettytable::Cell::new("NAMESPACE"),
        prettytable::Cell::new("STREAM"),
        prettytable::Cell::new("REPLICAS (MIN/MAX)"),
        prettytable::Cell::new("STATUS"),
        prettytable::Cell::new("PRESSURE"),
        prettytable::Cell::new("CREATED"),
        prettytable::Cell::new("UPDATED"),
    ]));

    let empty_vec = Vec::new();
    let processor_list = processors_response
        .get("processors")
        .and_then(Value::as_array)
        .unwrap_or(&empty_vec);

    // Process processors data
    for processor in processor_list {
        if let Value::Object(proc_obj) = processor {
            debug!("Processor: {:?}", proc_obj);

            let metadata = proc_obj.get("metadata").and_then(Value::as_object);
            let status_obj = proc_obj.get("status").and_then(Value::as_object);

            let id = metadata
                .and_then(|m| m.get("id"))
                .and_then(Value::as_str)
                .unwrap_or("N/A");
            let name = metadata
                .and_then(|m| m.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("N/A");
            let namespace = metadata
                .and_then(|m| m.get("namespace"))
                .and_then(Value::as_str)
                .unwrap_or("N/A");
            let stream = proc_obj
                .get("stream")
                .and_then(Value::as_str)
                .unwrap_or("N/A");

            let min_replicas_val = proc_obj.get("min_replicas").and_then(Value::as_i64);
            let max_replicas_val = proc_obj.get("max_replicas").and_then(Value::as_i64);

            let replicas_str = match (min_replicas_val, max_replicas_val) {
                (Some(min), Some(max)) => format!("{}/{}", min, max),
                (Some(min), None) => format!("{}/N/A", min),
                (None, Some(max)) => format!("N/A/{}", max),
                (None, None) => "N/A".to_string(),
            };

            let status = status_obj
                .and_then(|s| s.get("status"))
                .and_then(Value::as_str)
                .unwrap_or("N/A");

            let pressure = status_obj
                .and_then(|s| s.get("pressure"))
                .and_then(Value::as_i64)
                .map(|p| p.to_string())
                .unwrap_or_else(|| "N/A".to_string());

            let created = metadata
                .and_then(|m| m.get("created_at"))
                .and_then(Value::as_i64)
                .map(|timestamp| {
                    DateTime::<Utc>::from_timestamp(timestamp, 0)
                        .unwrap_or_default()
                        .format("%Y-%m-%d %H:%M:%S")
                        .to_string()
                })
                .unwrap_or_else(|| "N/A".to_string());

            let updated = metadata
                .and_then(|m| m.get("updated_at"))
                .and_then(Value::as_i64)
                .map(|timestamp| {
                    DateTime::<Utc>::from_timestamp(timestamp, 0)
                        .unwrap_or_default()
                        .format("%Y-%m-%d %H:%M:%S")
                        .to_string()
                })
                .unwrap_or_else(|| "N/A".to_string());

            // Add row to table
            table.add_row(prettytable::Row::new(vec![
                prettytable::Cell::new(id),
                prettytable::Cell::new(name),
                prettytable::Cell::new(namespace),
                prettytable::Cell::new(stream),
                prettytable::Cell::new(&replicas_str),
                prettytable::Cell::new(status),
                prettytable::Cell::new(&pressure),
                prettytable::Cell::new(&created),
                prettytable::Cell::new(&updated),
            ]));
        }
    }

    // Set table format and print
    table.set_format(*prettytable::format::consts::FORMAT_CLEAN);
    table.printstd();

    Ok(())
}

// Function to recursively remove null values from serde_json::Value
fn remove_null_values(value: &mut Value) {
    match value {
        Value::Object(map) => {
            // Collect keys with null values
            let keys_with_nulls: Vec<_> = map
                .iter()
                .filter_map(|(k, v)| if v.is_null() { Some(k.clone()) } else { None })
                .collect();

            // Remove keys with null values
            for k in keys_with_nulls {
                map.remove(&k);
            }

            // Recursively process the remaining values
            for v in map.values_mut() {
                remove_null_values(v);
            }
        }
        Value::Array(arr) => {
            // Recursively process each item in the array
            for v in arr.iter_mut() {
                remove_null_values(v);
            }
        }
        _ => {}
    }
}

pub async fn get_platforms() -> Result<(), Box<dyn Error>> {
    let platforms = vec!["gce", "runpod", "ec2"];
    println!("{:?}", platforms);
    Ok(())
}
