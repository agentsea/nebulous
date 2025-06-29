use crate::config::SERVER_CONFIG;
use sea_orm::{ConnectOptions, ConnectionTrait, Database, DatabaseConnection, DbErr, Schema};
use std::time::Duration;

pub type DbPool = DatabaseConnection;

// Helper function to create connection options
fn create_connect_options(url: String) -> ConnectOptions {
    let mut opt = ConnectOptions::new(url);

    // --- CONFIGURE POOL SIZE AND TIMEOUTS HERE ---
    opt.max_connections(80) // Example: Set pool size to 50
        .connect_timeout(Duration::from_secs(8))
        .acquire_timeout(Duration::from_secs(15)) // Increased acquire timeout slightly
        .idle_timeout(Duration::from_secs(60 * 5)) // Increased idle timeout (5 minutes)
        .max_lifetime(Duration::from_secs(60 * 10)); // Increased max lifetime (10 minutes)
                                                     // .sqlx_logging(true); // Uncomment for more verbose SQL logs if needed

    opt
}

pub async fn init_db() -> Result<DbPool, DbErr> {
    let database_url = &SERVER_CONFIG.database_url;
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

                    // --- Use ConnectOptions for fallback ---
                    let opts = create_connect_options(fallback_url);
                    let db = Database::connect(opts).await?;
                    // --------------------------------------

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

    // --- Use ConnectOptions for primary URL ---
    let opts = create_connect_options(database_url.clone()); // Clone if needed or ensure ownership
    let db = Database::connect(opts).await?;
    // -----------------------------------------

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
