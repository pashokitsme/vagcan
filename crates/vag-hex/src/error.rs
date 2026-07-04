use vag_transport::TransportError;

/// Errors from opening and driving the physical HEX cable.
#[derive(thiserror::Error, Debug)]
pub enum HexError {
    /// No cable enumerated on the bus.
    #[error("cable not found")]
    NotFound,
    /// The init handshake did not match what the cable was expected to answer.
    #[error("handshake mismatch: {0}")]
    Handshake(String),
    /// Underlying byte-pipe I/O failure (USB / serial).
    #[error("io error: {0}")]
    Io(String),
    /// Cable did not answer within the configured timeout.
    #[error("timeout")]
    Timeout,
    /// Cable envelope was malformed (bad length / checksum / escaping).
    #[error("framing error: {0}")]
    Framing(String),
    /// A layer whose wire format is not yet recovered from the capture.
    #[error("not implemented until capture defines it: {0}")]
    Unspecified(&'static str),
}

/// Bridge cable errors into the transport error the UDS stack consumes.
impl From<HexError> for TransportError {
    fn from(e: HexError) -> Self {
        match e {
            HexError::NotFound => TransportError::Disconnected,
            HexError::Timeout => TransportError::Timeout,
            HexError::Io(s) => TransportError::Io(s),
            HexError::Handshake(s) | HexError::Framing(s) => TransportError::Protocol(s),
            HexError::Unspecified(s) => TransportError::Protocol(s.to_string()),
        }
    }
}
