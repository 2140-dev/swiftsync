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
};

use crate::{
    ConnectionBuilder, ConnectionContext, HandshakeError, Negotiation, ParseMessageError,
    ReadContext, ReadHalf, WriteContext, WriteHalf, interpret_first_message, make_version,
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
        let mut tcp_stream = TcpStream::connect(socket_addr).await?;
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
        write_message(&mut tcp_stream, version, &mut write_half).await?;
        let their_version = read_half.read_message(&mut tcp_stream).await?;
        match their_version {
            Some(version) => {
                interpret_first_message(version, nonce, self.their_version, self.their_services)
                    .map_err(TokioConnectionError::Protocol)?;
            }
            None => return Err(TokioConnectionError::Protocol(HandshakeError::BadDecoy)),
        };
        // Send the services we offer
        if self.offer.addrv2 {
            write_message(&mut tcp_stream, NetworkMessage::SendAddrV2, &mut write_half).await?;
        }
        if self.offer.wtxid_relay {
            write_message(&mut tcp_stream, NetworkMessage::WtxidRelay, &mut write_half).await?;
        }
        negotiate_handshake(&mut tcp_stream, &mut read_half, &mut negotiation).await?;
        write_message(&mut tcp_stream, NetworkMessage::Verack, &mut write_half).await?;
        if self.offer.cmpct_block {
            let send_cmpct = NetworkMessage::SendCmpct(SendCmpct {
                send_compact: self.offer.cmpct_block,
                version: 0x02,
            });
            write_message(&mut tcp_stream, send_cmpct, &mut write_half).await?;
        }
        if self.offer.send_headers {
            write_message(
                &mut tcp_stream,
                NetworkMessage::SendHeaders,
                &mut write_half,
            )
            .await?;
        }
        let context =
            ConnectionContext::new(write_half, read_half, negotiation, self.their_services);
        Ok((tcp_stream, context))
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
    ) -> Result<Option<NetworkMessage>, ReadError>;
}

impl TokioTransportExt for ReadHalf {
    async fn read_message<R: AsyncReadExt + Send + Sync + Unpin>(
        &mut self,
        reader: &mut R,
    ) -> Result<Option<NetworkMessage>, ReadError> {
        match self {
            Self::V1(magic) => {
                let mut message_buf = vec![0_u8; 24];
                let _ = reader.read_exact(&mut message_buf).await?;
                let header: crate::MessageHeader = consensus::deserialize_partial(&message_buf)
                    .map_err(ParseMessageError::Consensus)?
                    .0;
                if header.magic != *magic {
                    return Err(ParseMessageError::UnexpectedMagic {
                        want: *magic,
                        got: header.magic,
                    }
                    .into());
                }
                if header.length > crate::MAX_MESSAGE_SIZE {
                    return Err(ParseMessageError::AbsurdSize {
                        message_size: header.length,
                    }
                    .into());
                }
                let mut contents_buf = vec![0_u8; header.length as usize];
                let _ = reader.read_exact(&mut contents_buf).await?;
                message_buf.extend_from_slice(&contents_buf);
                let message: RawNetworkMessage =
                    consensus::deserialize(&message_buf).map_err(ParseMessageError::Deserialize)?;
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
    write_message(writer, message, &mut ctx.write_half).await?;
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

// Error implementation section
#[derive(Debug)]
pub enum TokioConnectionError {
    Io(io::Error),
    Protocol(HandshakeError),
    Reader(ReadError),
}

impl From<io::Error> for TokioConnectionError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<ReadError> for TokioConnectionError {
    fn from(value: ReadError) -> Self {
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
    ParseMessageError(ParseMessageError),
    Io(io::Error),
    MessageMalformed,
}

impl Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ParseMessageError(r) => write!(f, "{r}"),
            Self::MessageMalformed => write!(f, "message data is malformed"),
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
