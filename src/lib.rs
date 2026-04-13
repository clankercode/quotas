pub mod auth;
pub mod cache;
pub mod config;
pub mod output;
pub mod providers;
pub mod tui;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("provider error: {0}")]
    Provider(String),

    #[error("auth error: {0}")]
    Auth(String),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("no credentials found for provider: {0}")]
    NoCredentials(String),

    #[error("internal error: {0}")]
    Internal(String),
}

pub type Result<T> = std::result::Result<T, Error>;
