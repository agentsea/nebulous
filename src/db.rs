// src/db.rs
use crate::config::CONFIG;
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbErr, Schema};

pub type DbPool = DatabaseConnection;

pub async fn init_db() -> Result<DbPool, DbErr> {
    let database_url = &CONFIG.database_url;
    println!("Connecting to database at: {}", database_url);

    // Create the data directory if it doesn't exist
    if database_url.starts_with("sqlite:") {
        let path = database_url.trim_start_matches("sqlite:");
        let path = std::path::Path::new(path);
        if let Some(parent) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                println!(
                    "Warning: Failed to create database directory at {}: {}",
                    parent.display(),
                    e
                );

                // Try to use a fallback location in the user's home directory
                // Using the same directory structure as in config.rs
                if let Some(home_dir) = dirs::home_dir() {
                    let fallback_dir = home_dir.join(".agentsea/data");
                    println!(
                        "Attempting to use fallback directory: {}",
                        fallback_dir.display()
                    );

                    if let Err(e2) = std::fs::create_dir_all(&fallback_dir) {
                        return Err(DbErr::Custom(format!(
                            "Failed to create both default and fallback database directories: {} and {}", 
                            e, e2
                        )));
                    }

                    // Use the fallback database path
                    let fallback_db_path = fallback_dir.join("nebu.db");
                    let fallback_url = format!("sqlite:{}", fallback_db_path.display());
                    println!("Using fallback database URL: {}", fallback_url);

                    let db = Database::connect(&fallback_url).await?;
                    create_tables(&db).await?;
                    return Ok(db);
                } else {
                    return Err(DbErr::Custom(format!(
                        "Failed to create database directory and couldn't find home directory: {}",
                        e
                    )));
                }
            }
        }
    }

    let db = Database::connect(database_url).await?;
    create_tables(&db).await?;
    Ok(db)
}

// Extract table creation logic to avoid duplication
async fn create_tables(db: &DbPool) -> Result<(), DbErr> {
    // Create the schema builder
    let schema = Schema::new(db.get_database_backend());

    // Create tables if they don't exist
    db.execute(
        db.get_database_backend().build(
            schema
                .create_table_from_entity(crate::entities::containers::Entity)
                .if_not_exists(),
        ),
    )
    .await?;

    // Create secrets table if it doesn't exist
    db.execute(
        db.get_database_backend().build(
            schema
                .create_table_from_entity(crate::entities::secrets::Entity)
                .if_not_exists(),
        ),
    )
    .await?;

    db.execute(
        db.get_database_backend().build(
            schema
                .create_table_from_entity(crate::entities::processors::Entity)
                .if_not_exists(),
        ),
    )
    .await?;

    db.execute(
        db.get_database_backend().build(
            schema
                .create_table_from_entity(crate::auth::db::Entity)
                .if_not_exists(),
        ),
    )
    .await?;

    db.execute(
        db.get_database_backend().build(
            schema
                .create_table_from_entity(crate::entities::volumes::Entity)
                .if_not_exists(),
        ),
    )
    .await?;

    db.execute(
        db.get_database_backend().build(
            schema
                .create_table_from_entity(crate::entities::namespaces::Entity)
                .if_not_exists(),
        ),
    )
    .await?;

    Ok(())
}
