use std::error::Error as StdError;
use std::fmt::{Display, Formatter};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProtocolError {
    pub code: String,
    pub message: String,
    pub status: Option<u16>,
    pub retryable: bool,
}

impl ProtocolError {
    pub fn new(
        code: impl Into<String>,
        message: impl Into<String>,
        status: Option<u16>,
        retryable: bool,
    ) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            status,
            retryable,
        }
    }
}

impl Display for ProtocolError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self.status {
            Some(status) => write!(
                formatter,
                "durable-worker {} (HTTP {}): {}",
                self.code, status, self.message
            ),
            None => write!(
                formatter,
                "durable-worker {}: {}",
                self.code, self.message
            ),
        }
    }
}

impl StdError for ProtocolError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransportError {
    pub message: String,
    pub retryable: bool,
}

impl TransportError {
    pub fn new(message: impl Into<String>, retryable: bool) -> Self {
        Self {
            message: message.into(),
            retryable,
        }
    }
}

impl Display for TransportError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "durable-worker transport error: {}", self.message)
    }
}

impl StdError for TransportError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DurableWorkerError {
    Protocol(ProtocolError),
    LeaseLost(ProtocolError),
    Transport(TransportError),
    Configuration(String),
    Serialization(String),
    WorkerJoin(String),
}

impl DurableWorkerError {
    pub fn retryable(&self) -> bool {
        match self {
            Self::Protocol(error) | Self::LeaseLost(error) => error.retryable,
            Self::Transport(error) => error.retryable,
            Self::Configuration(_) | Self::Serialization(_) | Self::WorkerJoin(_) => false,
        }
    }

    pub fn status(&self) -> Option<u16> {
        match self {
            Self::Protocol(error) | Self::LeaseLost(error) => error.status,
            _ => None,
        }
    }

    pub fn is_lease_lost(&self) -> bool {
        matches!(self, Self::LeaseLost(_))
    }
}

impl Display for DurableWorkerError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Protocol(error) | Self::LeaseLost(error) => Display::fmt(error, formatter),
            Self::Transport(error) => Display::fmt(error, formatter),
            Self::Configuration(message) => {
                write!(formatter, "durable-worker configuration error: {message}")
            }
            Self::Serialization(message) => {
                write!(formatter, "durable-worker serialization error: {message}")
            }
            Self::WorkerJoin(message) => {
                write!(formatter, "durable-worker task join error: {message}")
            }
        }
    }
}

impl StdError for DurableWorkerError {}
