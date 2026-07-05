use vag_transport::TransportError;

/// Errors from a raw CAN backend (adapter I/O + frame codec).
#[derive(thiserror::Error, Debug)]
pub enum CanError {
    #[error("io error: {0}")]
    Io(String),
    #[error("timeout")]
    Timeout,
    #[error("disconnected")]
    Disconnected,
    #[error("malformed frame: {0}")]
    MalformedFrame(String),
    #[error("not supported: {0}")]
    Unsupported(&'static str),
}

impl From<CanError> for TransportError {
    fn from(e: CanError) -> Self {
        match e {
            CanError::Io(s) => TransportError::Io(s),
            CanError::Timeout => TransportError::Timeout,
            CanError::Disconnected => TransportError::Disconnected,
            CanError::MalformedFrame(s) => TransportError::Protocol(s),
            CanError::Unsupported(s) => TransportError::Unsupported(s),
        }
    }
}
