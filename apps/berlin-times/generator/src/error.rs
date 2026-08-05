use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum GeneratorError {
    #[error("api request failed: {0}")]
    Api(String),
    #[error("api returned status {status}: {message}")]
    ApiStatus { status: u16, message: String },
    #[error("api refused the research request: {0}")]
    Refusal(String),
    #[error("configuration is invalid: {0}")]
    Config(String),
    #[error("failed to process image: {0}")]
    Image(String),
    #[error("input or output failed at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("json processing failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("no usable lead photo was found: {0}")]
    NoPhoto(String),
    #[error("remote request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("url is not safe to fetch: {0}")]
    UnsafeUrl(String),
    #[error("edition validation failed: {0}")]
    Validation(String),
}

pub type Result<T> = std::result::Result<T, GeneratorError>;

pub fn io_error(path: impl Into<PathBuf>) -> impl FnOnce(std::io::Error) -> GeneratorError {
    let path = path.into();
    move |source| GeneratorError::Io { path, source }
}
