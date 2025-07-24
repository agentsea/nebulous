use tracing::{debug, error, info, warn};

/// Structured logging macro for CLI user feedback
#[macro_export]
macro_rules! cli_info {
    ($($arg:tt)*) => {
        tracing::info!(category = "cli", $($arg)*)
    };
}

/// Structured logging macro for CLI user feedback with fields
#[macro_export]
macro_rules! cli_info_with_fields {
    ($($field:ident = $value:expr),*; $($arg:tt)*) => {
        tracing::info!(category = "cli", $($field = $value,)* $($arg)*)
    };
}

/// Structured logging macro for CLI errors
#[macro_export]
macro_rules! cli_error {
    ($($arg:tt)*) => {
        tracing::error!(category = "cli", $($arg)*)
    };
}

/// Structured logging macro for CLI errors with fields
#[macro_export]
macro_rules! cli_error_with_fields {
    ($($field:ident = $value:expr),*; $($arg:tt)*) => {
        tracing::error!(category = "cli", $($field = $value,)* $($arg)*)
    };
}

/// Structured logging macro for sync operations
#[macro_export]
macro_rules! sync_info {
    ($($arg:tt)*) => {
        tracing::info!(category = "sync", $($arg)*)
    };
}

/// Structured logging macro for sync operations with fields
#[macro_export]
macro_rules! sync_info_with_fields {
    ($($field:ident = $value:expr),*; $($arg:tt)*) => {
        tracing::info!(category = "sync", $($field = $value,)* $($arg)*)
    };
}

/// Structured logging macro for container operations
#[macro_export]
macro_rules! container_info {
    ($($arg:tt)*) => {
        tracing::info!(category = "container", $($arg)*)
    };
}

/// Structured logging macro for container operations with fields
#[macro_export]
macro_rules! container_info_with_fields {
    ($($field:ident = $value:expr),*; $($arg:tt)*) => {
        tracing::info!(category = "container", $($field = $value,)* $($arg)*)
    };
}

/// Structured logging macro for database operations
#[macro_export]
macro_rules! db_info {
    ($($arg:tt)*) => {
        tracing::info!(category = "database", $($arg)*)
    };
}

/// Structured logging macro for server operations
#[macro_export]
macro_rules! server_info {
    ($($arg:tt)*) => {
        tracing::info!(category = "server", $($arg)*)
    };
}

/// Structured logging macro for server operations with fields
#[macro_export]
macro_rules! server_info_with_fields {
    ($($field:ident = $value:expr),*; $($arg:tt)*) => {
        tracing::info!(category = "server", $($field = $value,)* $($arg)*)
    };
}

/// Helper function to convert println! style output to structured logging
pub fn log_stdout(message: &str) {
    tracing::info!(category = "stdout", message = %message);
}

/// Helper function to convert eprintln! style output to structured logging
pub fn log_stderr(message: &str) {
    tracing::error!(category = "stderr", message = %message);
} 