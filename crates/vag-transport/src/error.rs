#[derive(thiserror::Error, Debug)]
pub enum TransportError {
    #[error("io error: {0}")]
    Io(String),
    #[error("timeout")]
    Timeout,
    #[error("disconnected")]
    Disconnected,
    #[error("not supported: {0}")]
    Unsupported(&'static str),
    #[error("protocol error: {0}")]
    Protocol(String),
}
