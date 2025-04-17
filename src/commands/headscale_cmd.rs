use futures::{StreamExt, TryStreamExt};
use k8s_openapi::api::core::v1::Pod;
use kube::api::{AttachParams, AttachedProcess};
use kube::{Api, Client};

#[derive(Debug, Clone)]
struct HeadscalePod {
    name: String,
    namespace: String,
}

impl HeadscalePod {
    fn new() -> Self {
        let name = std::env::var("HEADSCALE_POD_NAME").expect("HEADSCALE_POD_NAME not set");
        let namespace =
            std::env::var("HEADSCALE_POD_NAMESPACE").expect("HEADSCALE_POD_NAMESPACE not set");
        HeadscalePod { name, namespace }
    }
}

async fn get_output(mut attached: AttachedProcess) -> String {
    let stdout = tokio_util::io::ReaderStream::new(attached.stdout().unwrap());
    let out = stdout
        .filter_map(|r| async { r.ok().and_then(|v| String::from_utf8(v.to_vec()).ok()) })
        .collect::<Vec<_>>()
        .await
        .join("");
    attached.join().await.unwrap();
    out
}

async fn headscale_cmd(cmd: Vec<&str>) -> Result<String, Box<dyn std::error::Error>> {
    let headscale_pod = HeadscalePod::new();

    let client = Client::try_default().await?;
    let api: Api<Pod> = Api::namespaced(client, headscale_pod.namespace.as_str());

    let attached = api
        .exec(
            headscale_pod.name.as_str(),
            cmd,
            &AttachParams::default().stderr(false),
        )
        .await?;
    let output = get_output(attached).await;
    Ok(output)
}

pub async fn create_api_key(expiration: &str) -> Result<String, Box<dyn std::error::Error>> {
    let cmd = vec!["headscale", "apikeys", "create", "--expiration", expiration];
    let api_key = headscale_cmd(cmd.into()).await?;
    Ok(api_key)
}

pub async fn validate_api_key(prefix: &str) -> Result<bool, Box<dyn std::error::Error>> {
    let cmd = vec!["headscale", "apikeys", "list", "-o", "json"];
    let output = headscale_cmd(cmd).await?;
    let api_keys = serde_json::from_str::<Vec<serde_json::Value>>(&output)?;

    let cmd = vec!["date", "+%s%N"];
    let timestamp = headscale_cmd(cmd).await?;

    for apikey in api_keys {
        if let Some(other_prefix) = apikey.get("prefix") {
            if other_prefix.as_str() == Some(prefix) {
                println!("API key with prefix {} found", prefix);

                return if let Some(expiration) = apikey.get("expiration") {
                    let seconds = expiration
                        .get("seconds")
                        .expect("API key has no expiration seconds");
                    let nanos = expiration
                        .get("nanos")
                        .expect("API key has no expiration nanos");
                    let expiration_time =
                        seconds.as_i64().unwrap() * 1_000_000_000 + nanos.as_i64().unwrap();

                    if expiration_time < timestamp.parse::<i64>().unwrap() {
                        println!("API key has expired");
                        Ok(false)
                    } else {
                        println!("API key is valid");
                        Ok(true)
                    }
                } else {
                    Err(format!("API key with prefix {} has no expiration time", prefix).into())
                };
            }
        }
    }
    println!("API key with prefix {} not found", prefix);
    Ok(false)
}

pub async fn revoke_api_key(prefix: &str) -> Result<(), Box<dyn std::error::Error>> {
    let cmd = vec!["headscale", "apikeys", "expire", "--prefix", prefix];
    headscale_cmd(cmd).await?;
    Ok(())
}
