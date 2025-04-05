use std::error::Error;

pub async fn list_api_keys() -> Result<(), Box<dyn Error>> {
    Ok(())
}

pub async fn generate_api_key() -> Result<(), Box<dyn Error>> {
    Ok(())
}

pub async fn revoke_api_key(id: &str) -> Result<(), Box<dyn Error>> {
    Ok(())
}
