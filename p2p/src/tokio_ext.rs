use::std::fmt::{Debug, Display};
use std::net::SocketAddr;

use tokio::{io, net::TcpStream, time::timeout};

use crate::{ConnectionBuilder, HandshakeError};

pub trait ConnectionTokioExt {
    type Error: Debug + Display + Send + Sync + std::error::Error;

    async fn open_connection(self, to: impl Into<SocketAddr>) -> Result<TcpStream, Self::Error>;
}

impl ConnectionTokioExt for ConnectionBuilder {
    type Error = TokioConnectionError;

    async fn open_connection(self, to: impl Into<SocketAddr>) -> Result<TcpStream, Self::Error> {
        let socket_addr = to.into();
        let tcp_stream = timeout(self.connection_timeout, TcpStream::connect(socket_addr)).await.map_err(|_| TokioConnectionError::Protocol(HandshakeError::TimedOut))??;
        Ok(tcp_stream)
    }
}

#[derive(Debug)]
pub enum TokioConnectionError {
    Io(io::Error),
    Protocol(HandshakeError),
}

impl From<io::Error> for TokioConnectionError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl Display for TokioConnectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(io) => write!(f, "unexpected io error: {io}"),
            Self::Protocol(proto) => write!(f, "protocol negotiation error: {proto}"),
        }
    }
}

impl std::error::Error for TokioConnectionError {};
