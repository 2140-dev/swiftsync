use std::{
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use bitcoin::{
    FeeRate, Network, consensus,
    p2p::{
        Address, Magic, ServiceFlags,
        message::{CommandString, NetworkMessage, RawNetworkMessage},
        message_network::VersionMessage,
    },
};
use validation::ValidationExt;

#[cfg(feature = "tokio")]
pub mod tokio_ext;

mod validation;

pub const MAX_MESSAGE_SIZE: u32 = 1024 * 1024 * 32;
pub const DEFAULT_USER_AGENT: &str = "/swiftsync:0.1.0/";
const LOCAL_HOST: Ipv4Addr = Ipv4Addr::new(0, 0, 0, 0);
const UNREACHABLE: SocketAddr = SocketAddr::V4(SocketAddrV4::new(LOCAL_HOST, 0));

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, std::hash::Hash)]
pub struct ProtocolVerison(pub u32);

impl ProtocolVerison {
    /// Support for relaying transactions by WTXID
    pub const WTXID_RELAY: ProtocolVerison = ProtocolVerison(70016);
    /// Invalid compact blocks are not a ban
    pub const NO_BAN_CMPCT: ProtocolVerison = ProtocolVerison(70015);
    /// Compact block message support
    pub const CMPCT_BLOCKS: ProtocolVerison = ProtocolVerison(70014);
    /// Support the `feefilter` message
    pub const FEE_FILTER: ProtocolVerison = ProtocolVerison(70013);
    /// Support `sendheaders` message to advertise new blocks with `header` messages
    pub const SEND_HEADERS: ProtocolVerison = ProtocolVerison(70012);
    /// Support NODE_BLOOM messages and do not support bloom filter messages if not set
    pub const NODE_BLOOM: ProtocolVerison = ProtocolVerison(70011);
    /// Support `reject` messages
    pub const REJECT: ProtocolVerison = ProtocolVerison(70002);
    /// Support bloom filter messages
    pub const BLOOM_FILTERS: ProtocolVerison = ProtocolVerison(70001);
    /// Support `mempool` messages
    pub const MEMPOOL: ProtocolVerison = ProtocolVerison(60002);
    /// Support `ping` and `pong` messages
    pub const PING_PONG: ProtocolVerison = ProtocolVerison(60001);
}

#[derive(Debug)]
pub struct ConnectionContext {
    read_ctx: ReadContext,
    write_ctx: WriteContext,
}

impl ConnectionContext {
    fn new(
        write_half: WriteHalf,
        read_half: ReadHalf,
        negotiation: Negotiation,
        their_services: ServiceFlags,
    ) -> Self {
        let read_ctx = ReadContext {
            read_half,
            negotiation,
            fee_filter: FeeRate::BROADCAST_MIN,
            last_message: Instant::now(),
            final_alert: false,
            addrs_received: 0,
        };
        let write_ctx = WriteContext {
            write_half,
            negotiation,
            their_services,
        };
        Self {
            read_ctx,
            write_ctx,
        }
    }

    pub fn into_split(self) -> (ReadContext, WriteContext) {
        (self.read_ctx, self.write_ctx)
    }

    pub fn addrs_received(&self) -> usize {
        self.read_ctx.addrs_received
    }

    pub fn last_message(&self) -> Instant {
        self.read_ctx.last_message
    }

    pub fn fee_filter(&self) -> FeeRate {
        self.read_ctx.fee_filter
    }
}

impl AsMut<ReadContext> for ConnectionContext {
    fn as_mut(&mut self) -> &mut ReadContext {
        &mut self.read_ctx
    }
}

impl AsMut<WriteContext> for ConnectionContext {
    fn as_mut(&mut self) -> &mut WriteContext {
        &mut self.write_ctx
    }
}

#[derive(Debug)]
pub struct ReadContext {
    read_half: ReadHalf,
    negotiation: Negotiation,
    fee_filter: FeeRate,
    final_alert: bool,
    last_message: Instant,
    addrs_received: usize,
}

impl ReadContext {
    pub fn addrs_received(&self) -> usize {
        self.addrs_received
    }

    pub fn last_message(&self) -> Instant {
        self.last_message
    }

    pub fn fee_filter(&self) -> FeeRate {
        self.fee_filter
    }

    fn ok_to_recv_message(&mut self, message: &NetworkMessage) -> bool {
        if matches!(
            message,
            NetworkMessage::FilterClear
                | NetworkMessage::FilterAdd(_)
                | NetworkMessage::FilterLoad(_)
                | NetworkMessage::WtxidRelay
                | NetworkMessage::SendAddrV2
                | NetworkMessage::MemPool
                | NetworkMessage::Verack
                | NetworkMessage::Version(_)
        ) {
            return false;
        }
        if matches!(message, NetworkMessage::Alert(_)) {
            if !self.final_alert {
                self.final_alert = true;
            } else {
                return false;
            }
        }
        if matches!(message, NetworkMessage::SendHeaders) {
            // No check for duplicate `sendheaders` for now
            self.negotiation.send_headers.them = true;
        }
        true
    }

    fn is_valid(&mut self, message: &NetworkMessage) -> bool {
        match &message {
            NetworkMessage::FeeFilter(f) => {
                if *f < 0 {
                    false
                } else {
                    let fee_rate = FeeRate::from_sat_per_kwu(*f as u32 / 4);
                    self.fee_filter = fee_rate;
                    true
                }
            }
            NetworkMessage::Headers(h) => h.is_valid(),
            NetworkMessage::GetData(r) => r.0.is_valid(),
            NetworkMessage::Inv(r) => r.0.is_valid(),
            _ => true,
        }
    }
}

#[derive(Debug)]
pub struct WriteContext {
    write_half: WriteHalf,
    negotiation: Negotiation,
    their_services: ServiceFlags,
}

impl WriteContext {
    fn ok_to_send(&self, message: &NetworkMessage) -> bool {
        if matches!(
            message,
            NetworkMessage::FilterClear
                | NetworkMessage::FilterAdd(_)
                | NetworkMessage::FilterLoad(_)
                | NetworkMessage::Alert(_)
                | NetworkMessage::WtxidRelay
                | NetworkMessage::SendAddrV2
                | NetworkMessage::SendCmpct(_)
                | NetworkMessage::SendHeaders
                | NetworkMessage::MemPool
                | NetworkMessage::Verack
                | NetworkMessage::Version(_)
        ) {
            return false;
        }
        if matches!(message, NetworkMessage::Addr(_)) && self.negotiation.addrv2.agree() {
            return false;
        }
        if matches!(
            message,
            NetworkMessage::BlockTxn(_) | NetworkMessage::CmpctBlock(_)
        ) && !self.negotiation.cmpct_block.agree()
        {
            return false;
        }
        if matches!(
            message,
            NetworkMessage::GetCFilters(_)
                | NetworkMessage::GetCFCheckpt(_)
                | NetworkMessage::GetCFHeaders(_)
        ) && !self.their_services.has(ServiceFlags::COMPACT_FILTERS)
        {
            return false;
        }
        true
    }
}

#[derive(Debug)]
pub struct ConnectionBuilder {
    network: Network,
    our_ip: SocketAddr,
    offered_services: ServiceFlags,
    their_services: ServiceFlags,
    our_version: ProtocolVerison,
    their_version: ProtocolVerison,
    offer: Offered,
    start_height: i32,
    user_agent: String,
}

impl ConnectionBuilder {
    pub fn new() -> Self {
        Self {
            network: Network::Bitcoin,
            our_ip: UNREACHABLE,
            offered_services: ServiceFlags::NONE,
            their_services: ServiceFlags::NONE,
            our_version: ProtocolVerison::WTXID_RELAY,
            their_version: ProtocolVerison::WTXID_RELAY,
            offer: Offered::default(),
            start_height: 0,
            user_agent: DEFAULT_USER_AGENT.to_string(),
        }
    }

    pub fn offered_services(self, us: ServiceFlags) -> Self {
        Self {
            offered_services: us,
            ..self
        }
    }

    pub fn their_services_expected(self, them: ServiceFlags) -> Self {
        Self {
            their_services: them,
            ..self
        }
    }

    pub fn downgrade_to_version(self, us: ProtocolVerison) -> Self {
        Self {
            our_version: us,
            ..self
        }
    }

    pub fn accept_minimum_version(self, them: ProtocolVerison) -> Self {
        Self {
            their_version: them,
            ..self
        }
    }

    pub fn add_start_height(self, start_height: i32) -> Self {
        Self {
            start_height,
            ..self
        }
    }

    pub fn set_user_agent(self, user_agent: String) -> Self {
        Self { user_agent, ..self }
    }

    pub fn change_network(self, network: Network) -> Self {
        Self { network, ..self }
    }

    pub fn no_cmpct_blocks(mut self) -> Self {
        self.offer.cmpct_block = false;
        Self { ..self }
    }

    pub fn announce_by_inv(mut self) -> Self {
        self.offer.send_headers = false;
        Self { ..self }
    }

    pub fn set_local_ip(self, us: SocketAddr) -> Self {
        Self { our_ip: us, ..self }
    }
}

impl Default for ConnectionBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
enum WriteHalf {
    V1(Magic),
}

impl WriteHalf {
    fn serialize_message(&mut self, message: NetworkMessage) -> Vec<u8> {
        match self {
            Self::V1(magic) => {
                let raw = RawNetworkMessage::new(*magic, message);
                consensus::serialize(&raw)
            }
        }
    }
}

#[derive(Debug)]
enum ReadHalf {
    V1(Magic),
}

#[derive(Debug, Clone, Copy, Default)]
struct Negotiation {
    wtxid_relay: TwoParty,
    addrv2: TwoParty,
    cmpct_block: TwoParty,
    send_headers: TwoParty,
}

#[derive(Debug, Clone, Copy, Default)]
struct TwoParty {
    us: bool,
    them: bool,
}

impl TwoParty {
    fn agree(&self) -> bool {
        self.us && self.them
    }
}

#[derive(Debug)]
struct Offered {
    wtxid_relay: bool,
    addrv2: bool,
    cmpct_block: bool,
    send_headers: bool,
}

impl Default for Offered {
    fn default() -> Self {
        Self {
            wtxid_relay: true,
            addrv2: true,
            cmpct_block: true,
            send_headers: true,
        }
    }
}

pub(crate) struct MessageHeader {
    magic: Magic,
    _command: CommandString,
    length: u32,
    _checksum: u32,
}

impl consensus::Decodable for MessageHeader {
    fn consensus_decode<R: bitcoin::io::BufRead + ?Sized>(
        reader: &mut R,
    ) -> Result<Self, bitcoin::consensus::encode::Error> {
        let magic = Magic::consensus_decode(reader)?;
        let _command = CommandString::consensus_decode(reader)?;
        let length = u32::consensus_decode(reader)?;
        let _checksum = u32::consensus_decode(reader)?;
        Ok(Self {
            magic,
            _command,
            length,
            _checksum,
        })
    }
}

fn make_version(
    version: ProtocolVerison,
    our_services: ServiceFlags,
    their_services: ServiceFlags,
    our_ip: SocketAddr,
    start_height: i32,
    user_agent: String,
    nonce: u64,
) -> VersionMessage {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let them = Address::new(&UNREACHABLE, their_services);
    let us = Address::new(&our_ip, our_services);
    VersionMessage {
        version: version.0,
        services: our_services,
        timestamp: now,
        receiver: them,
        sender: us,
        nonce,
        user_agent,
        start_height,
        relay: false,
    }
}

#[allow(clippy::result_large_err)]
fn interpret_first_message(
    message: NetworkMessage,
    nonce: u64,
    their_expected_version: ProtocolVerison,
    their_expected_services: ServiceFlags,
) -> Result<(), HandshakeError> {
    if let NetworkMessage::Version(version) = message {
        if version.nonce.eq(&nonce) {
            return Err(HandshakeError::ConnectedToSelf);
        }
        if version.version < their_expected_version.0 {
            return Err(HandshakeError::TooLowVersion(ProtocolVerison(
                version.version,
            )));
        }
        if !version.services.has(their_expected_services) {
            return Err(HandshakeError::UnsupportedFeature);
        }
    } else {
        return Err(HandshakeError::IrrelevantMessage(message));
    }
    Ok(())
}

#[derive(Debug)]
pub enum ParseMessageError {
    UnexpectedMagic { want: Magic, got: Magic },
    AbsurdSize { message_size: u32 },
    Consensus(consensus::ParseError),
    Deserialize(consensus::encode::DeserializeError),
}

impl From<consensus::ParseError> for ParseMessageError {
    fn from(value: bitcoin::consensus::ParseError) -> Self {
        Self::Consensus(value)
    }
}

impl From<consensus::encode::DeserializeError> for ParseMessageError {
    fn from(value: consensus::encode::DeserializeError) -> Self {
        Self::Deserialize(value)
    }
}

impl std::fmt::Display for ParseMessageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Deserialize(d) => write!(f, "{d}"),
            Self::Consensus(c) => write!(f, "{c}"),
            Self::AbsurdSize { message_size } => write!(f, "absurd message size: {message_size}"),
            Self::UnexpectedMagic { want, got } => write!(f, "expected magic: {want}, got: {got}"),
        }
    }
}

impl std::error::Error for ParseMessageError {}

#[derive(Debug, Clone)]
pub enum HandshakeError {
    TooLowVersion(ProtocolVerison),
    IrrelevantMessage(NetworkMessage),
    ConnectedToSelf,
    BadDecoy,
    UnsupportedFeature,
}

impl std::fmt::Display for HandshakeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IrrelevantMessage(m) => {
                write!(f, "unexpected message during handshake: {}", m.cmd())
            }
            Self::ConnectedToSelf => write!(f, "accidental connection to self"),
            Self::BadDecoy => write!(f, "expected a message but got a decoy"),
            Self::UnsupportedFeature => write!(
                f,
                "a feature we require is not supported by the connection."
            ),
            Self::TooLowVersion(version) => {
                write!(f, "the remote peer had a too-low version: {}", version.0)
            }
        }
    }
}

impl std::error::Error for HandshakeError {}

#[macro_export]
macro_rules! define_read_message_logic {
    ($awaiter:ident, $reader:expr, $magic:expr) => {{
        macro_rules! read_await {
            ($buffer:expr) => {
                $awaiter!($reader.read_exact($buffer))
            };
        }

        let mut message_buf = vec![0_u8; 24];
        read_await!(&mut message_buf)?;
        let header: $crate::MessageHeader = consensus::deserialize_partial(&message_buf)
            .map_err(ParseMessageError::Consensus)?
            .0;
        if header.magic != $magic {
            return Err($crate::ParseMessageError::UnexpectedMagic {
                want: header.magic,
                got: header.magic,
            }
            .into());
        }
        if header.length > $crate::MAX_MESSAGE_SIZE {
            return Err(ParseMessageError::AbsurdSize {
                message_size: header.length,
            }
            .into());
        }
        let mut contents_buf = vec![0_u8; header.length as usize];
        read_await!(&mut contents_buf)?;
        message_buf.extend_from_slice(&contents_buf);
        let message: RawNetworkMessage =
            consensus::deserialize(&message_buf).map_err(ParseMessageError::Deserialize)?;
        Ok(Some(message.into_payload()))
    }};
}

macro_rules! async_awaiter {
    ($e:expr) => {
        $e.await
    };
}

macro_rules! blocking_awaiter {
    ($e:expr) => {
        $e
    };
}

// The public-facing macros that users will call.
macro_rules! read_message_async {
    ($reader:expr, $magic:expr) => {
        $crate::define_read_message_logic!(async_awaiter, $reader, $magic)
    };
}

macro_rules! read_message_blocking {
    ($reader:expr) => {
        $crate::define_read_message_logic!(blocking_awaiter, $reader)
    };
}

pub(crate) use async_awaiter;
pub(crate) use blocking_awaiter;
pub(crate) use read_message_async;
pub(crate) use read_message_blocking;
