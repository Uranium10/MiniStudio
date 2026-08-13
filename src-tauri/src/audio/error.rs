use serde::Serialize;
use specta::Type;
use thiserror::Error;

#[derive(Clone, Debug, Error, Serialize, Type)]
#[serde(tag = "kind", content = "detail")]
pub enum EngineError {
    #[error("device not found: {0}")]
    DeviceNotFound(String),
    #[error("unsupported format: {0}")]
    UnsupportedFormat(String),
    #[error("audio command queue is full")]
    QueueFull,
    #[error("audio stream error: {0}")]
    Stream(String),
    #[error("asset error: {0}")]
    Asset(String),
    #[error("invalid request: {0}")]
    #[allow(dead_code)]
    InvalidRequest(String),
    #[error("native engine error: {0}")]
    Internal(String),
}

impl From<String> for EngineError {
    fn from(value: String) -> Self {
        let lower = value.to_ascii_lowercase();
        if lower.contains("device")
            && (lower.contains("not found") || lower.contains("unavailable"))
        {
            Self::DeviceNotFound(value)
        } else if lower.contains("unsupported") || lower.contains("format") {
            Self::UnsupportedFormat(value)
        } else if lower.contains("queue is full") {
            Self::QueueFull
        } else if lower.contains("stream") || lower.contains("backend") {
            Self::Stream(value)
        } else if lower.contains("asset") || lower.contains("audio file") {
            Self::Asset(value)
        } else {
            Self::Internal(value)
        }
    }
}

impl From<&str> for EngineError {
    fn from(value: &str) -> Self {
        Self::from(value.to_owned())
    }
}
