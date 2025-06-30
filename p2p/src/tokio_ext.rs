use ::std::fmt::{Debug, Display};
use std::net::SocketAddr;

use bitcoin::p2p::message::{NetworkMessage, RawNetworkMessage};
use bitcoin::p2p::message_compact_blocks::SendCmpct;
use bitcoin::secp256k1::rand;
use bitcoin::{consensus, p2p::Magic};
use tokio::io::AsyncWriteExt;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::{
    io::{self, AsyncReadExt},
    net::TcpStream,
    time::timeout,
};

use crate::{
    ConnectionBuilder, ConnectionContext, HandshakeError, Negotiation, ProtocolVerison,
    ReadContext, ReadHalf, WriteContext, WriteHalf, make_version,
};

pub trait TokioConnectionExt {
    type Error: Debug + Display + Send + Sync + std::error::Error;

    #[allow(async_fn_in_trait)]
    async fn open_connection(
        self,
        to: impl Into<SocketAddr>,
    ) -> Result<(TcpStream, ConnectionContext), Self::Error>;
}

impl TokioConnectionExt for ConnectionBuilder {
    type Error = TokioConnectionError;

    async fn open_connection(
        self,
        to: impl Into<SocketAddr>,
    ) -> Result<(TcpStream, ConnectionContext), Self::Error> {
        let socket_addr = to.into();
        let mut tcp_stream = timeout(self.connection_timeout, TcpStream::connect(socket_addr))
            .await
            .map_err(|_| TokioConnectionError::Protocol(HandshakeError::TimedOut))??;
        // Make a V2 connection here
        let mut negotiation = Negotiation::default();
        let magic = Magic::from_params(self.network);
        let mut write_half = WriteHalf::V1(magic);
        let mut read_half = ReadHalf::V1(magic);
        let nonce = rand::random();
        let version = NetworkMessage::Version(make_version(
            self.our_version,
            self.offered_services,
            self.their_services,
            self.our_ip,
            self.start_height,
            self.user_agent,
            nonce,
        ));
        let msg_bytes = write_half.serialize_message(version);
        tcp_stream.write_all(&msg_bytes).await?;
        tcp_stream.flush().await?;
        let their_version = read_half.read_message(&mut tcp_stream).await?;
        match their_version {
            Some(version) => {
                if let NetworkMessage::Version(version) = version {
                    if version.nonce.eq(&nonce) {
                        return Err(TokioConnectionError::Protocol(
                            HandshakeError::ConnectedToSelf,
                        ));
                    }
                    if version.version < self.their_version.0 {
                        return Err(TokioConnectionError::Protocol(
                            HandshakeError::TooLowVersion(ProtocolVerison(version.version)),
                        ));
                    }
                    if !version.services.has(self.their_services) {
                        return Err(TokioConnectionError::Protocol(
                            HandshakeError::UnsupportedFeature,
                        ));
                    }
                } else {
                    return Err(TokioConnectionError::Protocol(
                        HandshakeError::IrrelevantMessage(version),
                    ));
                }
            }
            None => return Err(TokioConnectionError::Protocol(HandshakeError::BadDecoy)),
        };
        // Send the services we offer
        if self.offer.addrv2 {
            let send_addrv2 = NetworkMessage::SendAddrV2;
            let msg_bytes = write_half.serialize_message(send_addrv2);
            tcp_stream.write_all(&msg_bytes).await?;
            tcp_stream.flush().await?;
        }
        if self.offer.wtxid_relay {
            let send_headers = NetworkMessage::WtxidRelay;
            let msg_bytes = write_half.serialize_message(send_headers);
            tcp_stream.write_all(&msg_bytes).await?;
            tcp_stream.flush().await?;
        }
        timeout(
            self.handshake_timeout,
            negotiate_handshake(&mut tcp_stream, &mut read_half, &mut negotiation),
        )
        .await
        .map_err(|_| TokioConnectionError::Protocol(HandshakeError::TimedOut))??;
        let msg_bytes = write_half.serialize_message(NetworkMessage::Verack);
        tcp_stream.write_all(&msg_bytes).await?;
        tcp_stream.flush().await?;
        if self.offer.cmpct_block {
            let send_headers = NetworkMessage::SendCmpct(SendCmpct {
                send_compact: self.offer.cmpct_block,
                version: 0x02,
            });
            let msg_bytes = write_half.serialize_message(send_headers);
            tcp_stream.write_all(&msg_bytes).await?;
            tcp_stream.flush().await?;
        }
        if self.offer.send_headers {
            let send_headers = NetworkMessage::SendHeaders;
            let msg_bytes = write_half.serialize_message(send_headers);
            tcp_stream.write_all(&msg_bytes).await?;
            tcp_stream.flush().await?;
        }
        let context =
            ConnectionContext::new(write_half, read_half, negotiation, self.their_services);
        Ok((tcp_stream, context))
    }
}

async fn negotiate_handshake(
    tcp_stream: &mut TcpStream,
    transport: &mut ReadHalf,
    negotiation: &mut Negotiation,
) -> Result<(), TokioConnectionError> {
    loop {
        let message = transport.read_message(tcp_stream).await?;
        match message {
            Some(message) => match message {
                NetworkMessage::SendAddrV2 => negotiation.addrv2.them = true,
                NetworkMessage::WtxidRelay => negotiation.wtxid_relay.them = true,
                NetworkMessage::SendCmpct(_) => negotiation.cmpct_block.them = true,
                NetworkMessage::SendHeaders => negotiation.send_headers.them = true,
                NetworkMessage::Verack => return Ok(()),
                other => {
                    return Err(TokioConnectionError::Protocol(
                        HandshakeError::IrrelevantMessage(other),
                    ));
                }
            },
            None => return Err(TokioConnectionError::Protocol(HandshakeError::BadDecoy)),
        }
    }
}

trait TokioTransportExt {
    #[allow(async_fn_in_trait)]
    async fn read_message<R: AsyncReadExt + Send + Sync + Unpin>(
        &mut self,
        reader: &mut R,
    ) -> Result<Option<NetworkMessage>, ReadMessageError>;
}

impl TokioTransportExt for ReadHalf {
    async fn read_message<R: AsyncReadExt + Send + Sync + Unpin>(
        &mut self,
        reader: &mut R,
    ) -> Result<Option<NetworkMessage>, ReadMessageError> {
        match self {
            Self::V1(magic) => {
                let mut message_buf = vec![0_u8; 24];
                let _ = reader.read_exact(&mut message_buf).await?;
                let header: crate::MessageHeader = consensus::deserialize_partial(&message_buf)?.0;
                if header.magic != *magic {
                    return Err(ReadMessageError::UnexpectedMagic {
                        want: *magic,
                        got: header.magic,
                    });
                }
                if header.length > crate::MAX_MESSAGE_SIZE {
                    return Err(ReadMessageError::AbsurdSize {
                        message_size: header.length,
                    });
                }
                let mut contents_buf = vec![0_u8; header.length as usize];
                let _ = reader.read_exact(&mut contents_buf).await?;
                message_buf.extend_from_slice(&contents_buf);
                let message: RawNetworkMessage = consensus::deserialize(&message_buf)?;
                Ok(Some(message.into_payload()))
            }
        }
    }
}

pub trait TokioWriteNetworkMessageExt {
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
    ) -> impl Future<Output = Result<(), WriteError>> {
        write_for_any(self, message, ctx)
    }
}

impl TokioWriteNetworkMessageExt for OwnedWriteHalf {
    fn write_message(
        &mut self,
        message: NetworkMessage,
        ctx: impl AsMut<WriteContext>,
    ) -> impl Future<Output = Result<(), WriteError>> {
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
    let msg_bytes = ctx.write_half.serialize_message(message);
    writer.write_all(&msg_bytes).await?;
    writer.flush().await?;
    Ok(())
}

pub trait TokioReadNetworkMessageExt {
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
                    return Err(ReadError::MessageMalformed);
                }
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
                    return Err(ReadError::MessageMalformed);
                }
                Ok(Some(message))
            }
            None => Ok(None),
        }
    }
}

#[allow(clippy::result_large_err)]
// Error implementation section
#[derive(Debug)]
pub enum TokioConnectionError {
    Io(io::Error),
    Protocol(HandshakeError),
    Reader(ReadMessageError),
}

impl From<io::Error> for TokioConnectionError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<ReadMessageError> for TokioConnectionError {
    fn from(value: ReadMessageError) -> Self {
        Self::Reader(value)
    }
}

impl Display for TokioConnectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(io) => write!(f, "{io}"),
            Self::Protocol(proto) => write!(f, "{proto}"),
            Self::Reader(read) => write!(f, "{read}"),
        }
    }
}

impl std::error::Error for TokioConnectionError {}

#[derive(Debug)]
pub enum WriteError {
    Io(io::Error),
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

#[derive(Debug)]
pub enum ReadError {
    NonsenseMessage(NetworkMessage),
    ReadMessageError(ReadMessageError),
    MessageMalformed,
}

impl Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ReadMessageError(r) => write!(f, "{r}"),
            Self::MessageMalformed => write!(f, "message data is malformed"),
            Self::NonsenseMessage(n) => write!(f, "{}", n.cmd()),
        }
    }
}

impl std::error::Error for ReadError {}

impl From<ReadMessageError> for ReadError {
    fn from(value: ReadMessageError) -> Self {
        Self::ReadMessageError(value)
    }
}

#[derive(Debug)]
pub enum ReadMessageError {
    UnexpectedMagic { want: Magic, got: Magic },
    AbsurdSize { message_size: u32 },
    Io(io::Error),
    Consensus(consensus::ParseError),
    Deserialize(consensus::encode::DeserializeError),
}

impl From<consensus::ParseError> for ReadMessageError {
    fn from(value: bitcoin::consensus::ParseError) -> Self {
        Self::Consensus(value)
    }
}

impl From<consensus::encode::DeserializeError> for ReadMessageError {
    fn from(value: consensus::encode::DeserializeError) -> Self {
        Self::Deserialize(value)
    }
}

impl From<io::Error> for ReadMessageError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl Display for ReadMessageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Deserialize(d) => write!(f, "{d}"),
            Self::Io(io) => write!(f, "{io}"),
            Self::Consensus(c) => write!(f, "{c}"),
            Self::AbsurdSize { message_size } => write!(f, "absurd message size: {message_size}"),
            Self::UnexpectedMagic { want, got } => write!(f, "expected magic: {want}, got: {got}"),
        }
    }
}

impl std::error::Error for ReadMessageError {}
