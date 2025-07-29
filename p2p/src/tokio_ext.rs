use ::std::fmt::{Debug, Display};
use std::net::SocketAddr;

use bitcoin::consensus;
use bitcoin::secp256k1::rand;
use p2p::message::{NetworkMessage, RawNetworkMessage};
use p2p::message_compact_blocks::SendCmpct;
use p2p::Magic;
use tokio::io::AsyncWriteExt;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::{
    io::{self, AsyncReadExt},
    net::TcpStream,
};

use crate::{
    async_awaiter, interpret_first_message, make_version, version_handshake_async,
    ConnectionBuilder, ConnectionContext, Feeler, HandshakeError, Negotiation, ParseMessageError,
    ReadContext, ReadHalf, WriteContext, WriteHalf,
};

/// Connect to peers using `tokio`.
pub trait TokioConnectionExt {
    type Error: Debug + Display + Send + Sync + std::error::Error;

    /// Open a TCP connection to a peer.
    #[allow(async_fn_in_trait)]
    async fn open_connection(
        self,
        to: impl Into<SocketAddr>,
    ) -> Result<(TcpStream, ConnectionContext), Self::Error>;

    /// Start a handshake with a pre-existing connection. Normally used after establishing a Socks5
    /// proxy connection.
    #[allow(async_fn_in_trait)]
    async fn start_handshake(
        self,
        tcp_stream: TcpStream,
    ) -> Result<(TcpStream, ConnectionContext), Self::Error>;

    /// Open a feeler to test a node's liveliness
    #[allow(async_fn_in_trait)]
    async fn open_feeler(self, to: impl Into<SocketAddr>) -> Result<Feeler, Self::Error>;
}

impl TokioConnectionExt for ConnectionBuilder {
    type Error = ConnectionError;

    async fn open_connection(
        mut self,
        to: impl Into<SocketAddr>,
    ) -> Result<(TcpStream, ConnectionContext), Self::Error> {
        let socket_addr = to.into();
        let timeout = tokio::time::timeout(self.tcp_timeout, TcpStream::connect(socket_addr)).await;
        let mut tcp_stream =
            timeout.map_err(|_| ConnectionError::Protocol(HandshakeError::Timeout))??;
        version_handshake_async!(tcp_stream, self)
    }

    async fn start_handshake(
        mut self,
        mut tcp_stream: TcpStream,
    ) -> Result<(TcpStream, ConnectionContext), Self::Error> {
        version_handshake_async!(tcp_stream, self)
    }

    async fn open_feeler(mut self, to: impl Into<SocketAddr>) -> Result<Feeler, Self::Error> {
        let socket_addr = to.into();
        let timeout = tokio::time::timeout(self.tcp_timeout, TcpStream::connect(socket_addr)).await;
        let mut tcp_stream =
            timeout.map_err(|_| ConnectionError::Protocol(HandshakeError::Timeout))??;
        let res: Result<(TcpStream, ConnectionContext), Self::Error> =
            version_handshake_async!(tcp_stream, self);
        let (_, ctx) = res?;
        let services = ctx.write_ctx.their_services;
        let protocol_version = ctx.write_ctx.their_protocol_verison;
        Ok(Feeler {
            services,
            protocol_version,
        })
    }
}

async fn write_message<W: AsyncWriteExt + Send + Sync + Unpin>(
    write: &mut W,
    message: NetworkMessage,
    write_half: &mut WriteHalf,
) -> Result<(), io::Error> {
    let msg_bytes = write_half.serialize_message(message);
    write.write_all(&msg_bytes).await?;
    write.flush().await?;
    Ok(())
}

trait TokioTransportExt {
    #[allow(async_fn_in_trait)]
    async fn read_message<R: AsyncReadExt + Send + Sync + Unpin>(
        &mut self,
        reader: &mut R,
    ) -> Result<Option<NetworkMessage>, ReadError>;
}

impl TokioTransportExt for ReadHalf {
    async fn read_message<R: AsyncReadExt + Send + Sync + Unpin>(
        &mut self,
        reader: &mut R,
    ) -> Result<Option<NetworkMessage>, ReadError> {
        match self {
            Self::V1(magic) => {
                crate::read_message_async!(reader, *magic)
            }
        }
    }
}

/// Write bitcoin network messages directly over `tokio` TCP streams.
pub trait TokioWriteNetworkMessageExt {
    /// Write a message with the current context.
    #[allow(async_fn_in_trait)]
    async fn write_message(
        &mut self,
        message: NetworkMessage,
        ctx: impl AsMut<WriteContext>,
    ) -> Result<(), WriteError>;
}

impl TokioWriteNetworkMessageExt for TcpStream {
    fn write_message(
        &mut self,
        message: NetworkMessage,
        ctx: impl AsMut<WriteContext>,
    ) -> impl std::future::Future<Output = Result<(), WriteError>> {
        write_for_any(self, message, ctx)
    }
}

impl TokioWriteNetworkMessageExt for OwnedWriteHalf {
    fn write_message(
        &mut self,
        message: NetworkMessage,
        ctx: impl AsMut<WriteContext>,
    ) -> impl std::future::Future<Output = Result<(), WriteError>> {
        write_for_any(self, message, ctx)
    }
}

async fn write_for_any<W: AsyncWriteExt + Send + Sync + Unpin>(
    writer: &mut W,
    message: NetworkMessage,
    mut ctx: impl AsMut<WriteContext>,
) -> Result<(), WriteError> {
    let ctx = ctx.as_mut();
    if !ctx.ok_to_send(&message) {
        return Err(WriteError::NotRecommended(message));
    };
    write_message(writer, message, &mut ctx.write_half).await?;
    Ok(())
}

/// Read a message directly off a TCP stream.
pub trait TokioReadNetworkMessageExt {
    /// Try to read a message and error otherwise.
    ///
    /// This method performs some light validation to ensure the node is not sending spam or
    /// non-sensical messages.
    #[allow(async_fn_in_trait)]
    async fn read_message(
        &mut self,
        ctx: impl AsMut<ReadContext>,
    ) -> Result<Option<NetworkMessage>, ReadError>;
}

impl TokioReadNetworkMessageExt for TcpStream {
    async fn read_message(
        &mut self,
        mut rtx: impl AsMut<ReadContext>,
    ) -> Result<Option<NetworkMessage>, ReadError> {
        let ctx = rtx.as_mut();
        let message = ctx.read_half.read_message(self).await?;
        match message {
            Some(message) => {
                if !ctx.ok_to_recv_message(&message) {
                    return Err(ReadError::NonsenseMessage(message));
                }
                if !ctx.is_valid(&message) {
                    return Err(ReadError::ParseMessageError(ParseMessageError::Malformed));
                }
                ctx.update_metadata(&message);
                Ok(Some(message))
            }
            None => Ok(None),
        }
    }
}

impl TokioReadNetworkMessageExt for OwnedReadHalf {
    async fn read_message(
        &mut self,
        mut rtx: impl AsMut<ReadContext>,
    ) -> Result<Option<NetworkMessage>, ReadError> {
        let ctx = rtx.as_mut();
        let message = ctx.read_half.read_message(self).await?;
        match message {
            Some(message) => {
                if !ctx.ok_to_recv_message(&message) {
                    return Err(ReadError::NonsenseMessage(message));
                }
                if !ctx.is_valid(&message) {
                    return Err(ReadError::ParseMessageError(ParseMessageError::Malformed));
                }
                ctx.update_metadata(&message);
                Ok(Some(message))
            }
            None => Ok(None),
        }
    }
}

// Error implementation section

/// Errors that may occur when starting a connection.
#[derive(Debug)]
pub enum ConnectionError {
    /// Read or write failure.
    Io(io::Error),
    /// The handshake failed to malformed messages or a mismatch in preferences.
    Protocol(HandshakeError),
    /// A message that was read violated the protocol.
    Reader(ReadError),
}

impl From<io::Error> for ConnectionError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<ReadError> for ConnectionError {
    fn from(value: ReadError) -> Self {
        Self::Reader(value)
    }
}

impl Display for ConnectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(io) => write!(f, "{io}"),
            Self::Protocol(proto) => write!(f, "{proto}"),
            Self::Reader(read) => write!(f, "{read}"),
        }
    }
}

impl std::error::Error for ConnectionError {}

/// Errors when attempting to write a message.
#[derive(Debug)]
pub enum WriteError {
    /// Writing to the stream failed.
    Io(io::Error),
    /// The message is invalid or not supported.
    NotRecommended(NetworkMessage),
}

impl From<io::Error> for WriteError {
    fn from(value: io::Error) -> Self {
        WriteError::Io(value)
    }
}

impl Display for WriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(io) => write!(f, "{io}"),
            Self::NotRecommended(msg) => write!(f, "non-sensical message: {}", msg.cmd()),
        }
    }
}

/// Errors when reading messages off of the stream.
#[derive(Debug)]
pub enum ReadError {
    /// The message violates the protocol. Normally, these are deprecated messages or messages that
    /// should have been sent during the handshake.
    NonsenseMessage(NetworkMessage),
    /// Parsing a message failed.
    ParseMessageError(ParseMessageError),
    /// The stream was closed.
    Io(io::Error),
}

impl Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ParseMessageError(r) => write!(f, "{r}"),
            Self::Io(io) => write!(f, "{io}"),
            Self::NonsenseMessage(n) => write!(f, "{}", n.cmd()),
        }
    }
}

impl std::error::Error for ReadError {}

impl From<ParseMessageError> for ReadError {
    fn from(value: ParseMessageError) -> Self {
        Self::ParseMessageError(value)
    }
}

impl From<io::Error> for ReadError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}
