use thiserror::Error;

#[derive(Debug, Error)]
pub enum StartupError {
    #[error("Configuration error: {0}")]
    ConfigError(String),

    #[error("Token signing misconfigured: {0}")]
    SigningMisconfigured(String),

    #[error("Key/Cert mismatch: {0}")]
    KeyCertMismatch(String),

    #[error("Other startup error: {0}")]
    Other(String),
}
