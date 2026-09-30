use thiserror::Error;

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Model request cancelled")]
    Cancelled,
    #[error("native worker supervisor is shutting down")]
    ShuttingDown,
    #[error("native worker stopped under active memory pressure: {0}")]
    ResourcePressure(String),
    #[error("Native worker is busy; drain requests before evicting its model")]
    WorkerBusy,
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, RuntimeError>;
