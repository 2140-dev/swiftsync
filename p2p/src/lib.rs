use std::{
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use bitcoin::{
    FeeRate, Network, consensus,
    p2p::{
        Address, Magic, ServiceFlags,
        message::{CommandString, NetworkMessage, RawNetworkMessage},
        message_network::VersionMessage,
    },
};

#[cfg(feature = "tokio")]
pub mod tokio_ext;

mod validation;

pub const MAX_MESSAGE_SIZE: u32 = 1024 * 1024 * 32;
pub const DEFAULT_USER_AGENT: &str = "/swiftsync:0.1.0/";
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(2);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
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
    transport: Transport,
    negotiation: Negotiation,
    offered: Offered,
    their_services: ServiceFlags,
    fee_filter: FeeRate,
    final_alert: bool,
    last_message: Instant,
    total_addrs: usize,
}

impl ConnectionContext {
    fn new(transport: Transport, negotiation: Negotiation, offered: Offered, their_services: ServiceFlags) -> Self {
        Self {
            transport,
            negotiation,
            offered,
            their_services,
            fee_filter: FeeRate::BROADCAST_MIN,
            final_alert: false,
            last_message: Instant::now(),
            total_addrs: 0,
        }
    }

    pub fn addrs_received(&self) -> usize {
        self.total_addrs
    }

    pub fn last_message(&self) -> Instant {
        self.last_message
    }

    pub fn fee_filter(&self) -> FeeRate {
        self.fee_filter
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
    handshake_timeout: Duration,
    connection_timeout: Duration,
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
            handshake_timeout: HANDSHAKE_TIMEOUT,
            connection_timeout: CONNECTION_TIMEOUT,
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

    pub fn connection_timeout(self, timeout: Duration) -> Self {
        Self {
            connection_timeout: timeout,
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
enum Transport {
    V1(Magic),
}

impl Transport {
    fn serialize_message(&mut self, message: NetworkMessage) -> Vec<u8> {
        match self {
            Self::V1(magic) => {
                let raw = RawNetworkMessage::new(*magic, message);
                consensus::serialize(&raw)
            }
        }
    }
}

#[derive(Debug, Clone, Default)]
struct Negotiation {
    wtxid_relay: bool,
    addrv2: bool,
    cmpct_block: bool,
    send_headers: bool,
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

#[derive(Debug, Clone)]
pub enum HandshakeError {
    TooLowVersion(ProtocolVerison),
    IrrelevantMessage(NetworkMessage),
    ConnectedToSelf,
    BadDecoy,
    UnsupportedFeature,
    TimedOut,
}

impl std::fmt::Display for HandshakeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TimedOut => write!(f, "the handshake timed out."),
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
