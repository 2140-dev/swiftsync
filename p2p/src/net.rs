use std::{
    io::{self, Read, Write},
    net::{SocketAddr, TcpStream},
};

use bitcoin::p2p::Magic;
use bitcoin::p2p::message::NetworkMessage;
use bitcoin::p2p::message::RawNetworkMessage;
use bitcoin::secp256k1::rand;
use bitcoin::{consensus, p2p::message_compact_blocks::SendCmpct};

use crate::{
    interpret_first_message, make_version, ConnectionBuilder, ConnectionContext, HandshakeError, Negotiation, ParseMessageError, ReadContext, ReadHalf, WriteContext, WriteHalf
};

/// Open a connection to a potential peer.
pub trait ConnectionExt {
    #[allow(clippy::result_large_err)]
    fn open_connection(
        self,
        to: impl Into<SocketAddr>,
    ) -> Result<(TcpStream, ConnectionContext), ConnectionError>;
}

impl ConnectionExt for ConnectionBuilder {
    fn open_connection(
        self,
        to: impl Into<SocketAddr>,
    ) -> Result<(TcpStream, ConnectionContext), ConnectionError> {
        let socket_addr = to.into();
        let mut tcp_stream = TcpStream::connect_timeout(&socket_addr, self.tcp_timeout)?;
        // Make V2 connection
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
        write_message(&mut tcp_stream, version, &mut write_half)?;
        let version = read_half.read_message(&mut tcp_stream)?;
        match version {
            Some(version) => {
                interpret_first_message(version, nonce, self.their_version, self.their_services)
                    .map_err(ConnectionError::Protocol)?;
            }
            None => {
                return Err(ConnectionError::Protocol(HandshakeError::BadDecoy));
            }
        }
        if self.offer.addrv2 {
            write_message(&mut tcp_stream, NetworkMessage::SendAddrV2, &mut write_half)?;
        }
        if self.offer.wtxid_relay {
            write_message(&mut tcp_stream, NetworkMessage::WtxidRelay, &mut write_half)?;
        }
        loop {
            let message = read_half.read_message(&mut tcp_stream)?;
            match message {
                Some(message) => match message {
                    NetworkMessage::Verack => break,
                    NetworkMessage::WtxidRelay => negotiation.wtxid_relay.them = true,
                    NetworkMessage::SendHeaders => negotiation.send_headers.them = true,
                    NetworkMessage::SendAddrV2 => negotiation.addrv2.them = true,
                    NetworkMessage::SendCmpct(_) => negotiation.cmpct_block.them = true,
                    other => {
                        return Err(ConnectionError::Protocol(
                            HandshakeError::IrrelevantMessage(other),
                        ));
                    }
                },
                None => continue,
            }
        }
        write_message(&mut tcp_stream, NetworkMessage::Verack, &mut write_half)?;
        if self.offer.cmpct_block {
            write_message(
                &mut tcp_stream,
                NetworkMessage::SendCmpct(SendCmpct {
                    version: 0x02,
                    send_compact: true,
                }),
                &mut write_half,
            )?;
        }
        if self.offer.send_headers {
            write_message(
                &mut tcp_stream,
                NetworkMessage::SendHeaders,
                &mut write_half,
            )?;
        }
        let context =
            ConnectionContext::new(write_half, read_half, negotiation, self.their_services);
        Ok((tcp_stream, context))
    }
}

#[allow(clippy::result_large_err)]
trait ReadHalfExt {
    fn read_message<R: Read + Send + Sync>(
        &mut self,
        reader: &mut R,
    ) -> Result<Option<NetworkMessage>, ReadError>;
}

impl ReadHalfExt for ReadHalf {
    fn read_message<R: Read + Send + Sync>(
        &mut self,
        reader: &mut R,
    ) -> Result<Option<NetworkMessage>, ReadError> {
        match self {
            Self::V1(magic) => {
                use crate::blocking_awaiter;
                crate::read_message_blocking!(reader, *magic)
            }
        }
    }
}

/// Write a network message directly to a stream.
#[allow(clippy::result_large_err)]
pub trait WriteExt {
    fn write_message(
        &mut self,
        message: NetworkMessage,
        ctx: impl AsMut<WriteContext>,
    ) -> Result<(), WriteError>;
}

impl WriteExt for TcpStream {
    fn write_message(
        &mut self,
        message: NetworkMessage,
        mut ctx: impl AsMut<WriteContext>,
    ) -> Result<(), WriteError> {
        let ctx = ctx.as_mut();
        if !ctx.ok_to_send(&message) {
            return Err(WriteError::NotRecommended(message));
        }
        write_message(self, message, &mut ctx.write_half).map_err(WriteError::Io)
    }
}

fn write_message<W: Write + Send + Sync>(
    tcp_stream: &mut W,
    message: NetworkMessage,
    write_half: &mut WriteHalf,
) -> Result<(), io::Error> {
    let msg_bytes = write_half.serialize_message(message);
    tcp_stream.write_all(&msg_bytes)?;
    tcp_stream.flush()?;
    Ok(())
}

/// Read a message directly off of the stream.
pub trait ReadExt {
    #[allow(clippy::result_large_err)]
    fn read_message(&mut self, ctx: impl AsMut<ReadContext>) -> Result<Option<NetworkMessage>, ReadError>;
}

impl ReadExt for TcpStream {
    fn read_message(&mut self, mut ctx: impl AsMut<ReadContext>) -> Result<Option<NetworkMessage>, ReadError> {
        let ctx = ctx.as_mut();
        let message = ctx.read_half.read_message(self)?;
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

// Error implementations

/// Errors occurring during an attempted connection.
#[derive(Debug)]
pub enum ConnectionError {
    Io(io::Error),
    Protocol(HandshakeError),
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

impl From<HandshakeError> for ConnectionError {
    fn from(value: HandshakeError) -> Self {
        Self::Protocol(value)
    }
}

pub enum WriteError {
    Io(io::Error),
    NotRecommended(NetworkMessage),
}

impl From<io::Error> for WriteError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
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
    /// The stream was closed or reset.
    Io(io::Error),
}

impl std::fmt::Display for ReadError {
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
