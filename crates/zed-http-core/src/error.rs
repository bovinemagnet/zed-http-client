use std::io;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum HttpClientError {
    #[error("invalid request line at line {line}: {content}")]
    InvalidRequestLine { line: usize, content: String },
    #[error("no request block found")]
    NoRequestFound,
    #[error("no request found for line {0}")]
    NoRequestForLine(usize),
    #[error("missing variable: {0}")]
    MissingVariable(String),
    #[error("{0}")]
    Message(String),
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}
