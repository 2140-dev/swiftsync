use std::time::Duration;

use bitcoin::p2p::ServiceFlags;

pub mod tokio_ext;

const CONNECTION_TIMEOUT: Duration = Duration::from_secs(1);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(2);

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
pub struct ConnectionBuilder {
    offered_services: ServiceFlags,
    their_services: ServiceFlags,
    our_version: ProtocolVerison,
    their_version: ProtocolVerison,
    they_sent_wtxid_relay: bool,
    they_sent_cmpct_block: bool,
    they_sent_send_headers: bool,
    they_sent_addrv2: bool,
    handshake_timeout: Duration,
    connection_timeout: Duration,
}

impl ConnectionBuilder {
    fn new() -> Self {
        Self {
            offered_services: ServiceFlags::NONE,
            their_services: ServiceFlags::NONE,
            our_version: ProtocolVerison::WTXID_RELAY,
            their_version: ProtocolVerison::WTXID_RELAY,
            they_sent_wtxid_relay: false,
            they_sent_cmpct_block: false,
            they_sent_addrv2: false,
            they_sent_send_headers: false,
            handshake_timeout: HANDSHAKE_TIMEOUT,
            connection_timeout: CONNECTION_TIMEOUT,
        }
    }

    fn offered_services(self, us: ServiceFlags) -> Self {
        Self {
            offered_services: us,
            ..self
        }
    }

    fn their_services_required(self, them: ServiceFlags) -> Self {
        Self {
            their_services: them,
            ..self
        }
    }

    fn downgrade_to_version(self, us: ProtocolVerison) -> Self {
        Self {
            our_version: us,
            ..self
        }
    }

    fn accept_minimum_version(self, them: ProtocolVerison) -> Self {
        Self {
            their_version: them,
            ..self
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum HandshakeError {
    TooLowVersion(ProtocolVerison),
    UnsupportedFeature,
    TimedOut,
}

impl std::fmt::Display for HandshakeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TimedOut => write!(f, "the handshake timed out."),
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
