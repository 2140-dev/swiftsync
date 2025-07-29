use std::{
    cmp::min,
    fmt::Display,
    net::{Ipv4Addr, SocketAddrV4},
    sync::atomic::{self, AtomicBool},
    time::{SystemTime, UNIX_EPOCH},
};

use bitcoin::Network;
use p2p::{
    message::{CommandString, NetworkMessage},
    message_compact_blocks::SendCmpct,
    message_network::{Alert, ClientSoftwareVersion, UserAgent, UserAgentVersion, VersionMessage},
    ProtocolVersion, ServiceFlags,
};

const CLIENT_VERSION: ClientSoftwareVersion = ClientSoftwareVersion::SemVer {
    major: 0,
    minor: 1,
    revision: 0,
};
const AGENT_VERSION: UserAgentVersion = UserAgentVersion::new(CLIENT_VERSION);
const CLIENT_NAME: &str = "SwiftSync";
const LOCAL_HOST_IP: Ipv4Addr = Ipv4Addr::new(0, 0, 0, 0);
const LOCAL_HOST: SocketAddrV4 = SocketAddrV4::new(LOCAL_HOST_IP, 0);
const NETWORK: Network = Network::Bitcoin;

#[derive(Debug, Clone)]
struct ConnectionConfig {
    our_version: ProtocolVersion,
    our_services: ServiceFlags,
    expected_version: ProtocolVersion,
    expected_services: ServiceFlags,
    send_cmpct: SendCmpct,
    user_agent: UserAgent,
    network: Network,
}

impl ConnectionConfig {
    fn new() -> Self {
        let user_agent = UserAgent::new(CLIENT_NAME, AGENT_VERSION);
        Self {
            our_version: ProtocolVersion::WTXID_RELAY_VERSION,
            our_services: ServiceFlags::NONE,
            expected_version: ProtocolVersion::WTXID_RELAY_VERSION,
            expected_services: ServiceFlags::NONE,
            send_cmpct: SendCmpct {
                send_compact: false,
                version: 0,
            },
            user_agent,
            network: NETWORK,
        }
    }

    fn change_network(mut self, network: Network) -> Self {
        self.network = network;
        self
    }

    fn decrease_version_requirement(mut self, protocol_version: ProtocolVersion) -> Self {
        self.expected_version = protocol_version;
        self
    }

    fn set_service_requirement(mut self, service_flags: ServiceFlags) -> Self {
        self.expected_services = service_flags;
        self
    }

    fn offer_services(mut self, service_flags: ServiceFlags) -> Self {
        self.our_services = service_flags;
        self
    }

    fn user_agent(mut self, user_agent: UserAgent) -> Self {
        self.user_agent = user_agent;
        self
    }

    fn send_cmpct(mut self, send_cmpct: SendCmpct) -> Self {
        self.send_cmpct = send_cmpct;
        self
    }

    fn start_handshake(
        self,
        network_message: NetworkMessage,
        origin: Origin,
    ) -> Result<InitializedHandshake, VersionError> {
        // The first message must always be a `version`
        let version = match network_message {
            NetworkMessage::Version(version) => version,
            e => return Err(VersionError::IrrelevantMessage(e.command())),
        };
        let time_received = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time went backwards");
        let mut suggested_messages = Vec::new();
        // Add the version message to the stack if we are receiving a version
        if let Origin::Inbound { nonce, version } = origin {
            if version.nonce.eq(&nonce) {
                return Err(VersionError::ConnectionToSelf);
            }
            suggested_messages.push(NetworkMessage::Version(version));
        }
        // Reject an incompatible peer based on our requirements
        if version.version < self.expected_version
            || version.version < ProtocolVersion::MIN_PEER_PROTO_VERSION
        {
            return Err(VersionError::TooLowVersion(version.version));
        }
        if !version.services.has(self.expected_services) {
            return Err(VersionError::NotEnoughServices(version.services));
        }
        // Prepare messages to send based on the effective version
        let effective_version = min(self.our_version, version.version);
        if effective_version > ProtocolVersion::WTXID_RELAY_VERSION {
            suggested_messages.push(NetworkMessage::WtxidRelay);
        }
        // Weird case where this number is not a constant in Bitcoin Core
        if effective_version > ProtocolVersion::from_nonstandard(70016) {
            suggested_messages.push(NetworkMessage::SendAddrV2);
        }
        if effective_version > ProtocolVersion::SENDHEADERS_VERSION {
            suggested_messages.push(NetworkMessage::SendHeaders);
        } else {
            suggested_messages.push(NetworkMessage::Alert(Alert::final_alert()));
        }
        let net_time_diff = time_received.as_secs_f64() as i64 - version.timestamp;
        let metadata = ConnectionMetadata {
            lowest_common_version: effective_version,
            their_services: version.services,
            net_time_difference: net_time_diff,
            reported_height: version.start_height,
            prefers_addrv2: AtomicBool::new(false),
            prefers_wtxid: AtomicBool::new(false),
            prefers_headers: AtomicBool::new(false),
            prefers_cmpct: AtomicBool::new(false),
        };
        let initial_handshake = InitializedHandshake {
            metadata,
            suggested_messages,
        };
        Ok(initial_handshake)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Origin {
    Inbound { nonce: u64, version: VersionMessage },
    OutBound,
}

impl Default for ConnectionConfig {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
struct ConnectionMetadata {
    lowest_common_version: ProtocolVersion,
    their_services: ServiceFlags,
    net_time_difference: i64,
    reported_height: i32,
    prefers_headers: AtomicBool,
    prefers_wtxid: AtomicBool,
    prefers_addrv2: AtomicBool,
    prefers_cmpct: AtomicBool,
}

// States

#[derive(Debug)]
struct InitializedHandshake {
    metadata: ConnectionMetadata,
    suggested_messages: Vec<NetworkMessage>,
}

impl InitializedHandshake {
    fn take_suggested(&mut self) -> impl Iterator<Item = NetworkMessage> {
        core::mem::take(&mut self.suggested_messages).into_iter()
    }

    fn negotiate(
        self,
        network_message: NetworkMessage,
    ) -> Result<NegotiationUpdate, NegotiationError> {
        match &network_message {
            NetworkMessage::Verack => Ok(NegotiationUpdate::Finished(CompletedHandshake {
                metadata: self.metadata,
                suggested_messages: Vec::new(),
            })),
            NetworkMessage::Alert(_) => {
                if self.metadata.lowest_common_version > ProtocolVersion::INVALID_CB_NO_BAN_VERSION
                {
                    return Err(NegotiationError::UnexpectedMessage(
                        network_message.command(),
                    ));
                }
                Ok(NegotiationUpdate::Updated(self))
            }
            NetworkMessage::WtxidRelay => {
                if self.metadata.lowest_common_version < ProtocolVersion::WTXID_RELAY_VERSION {
                    return Err(NegotiationError::UnexpectedMessage(
                        network_message.command(),
                    ));
                }
                self.metadata
                    .prefers_wtxid
                    .store(true, atomic::Ordering::Relaxed);
                Ok(NegotiationUpdate::Updated(self))
            }
            NetworkMessage::SendAddrV2 => {
                if self.metadata.lowest_common_version < ProtocolVersion::from_nonstandard(70016) {
                    return Err(NegotiationError::UnexpectedMessage(
                        network_message.command(),
                    ));
                }
                self.metadata
                    .prefers_addrv2
                    .store(true, atomic::Ordering::Relaxed);
                Ok(NegotiationUpdate::Updated(self))
            }
            NetworkMessage::SendHeaders => {
                if self.metadata.lowest_common_version < ProtocolVersion::SENDHEADERS_VERSION {
                    return Err(NegotiationError::UnexpectedMessage(
                        network_message.command(),
                    ));
                }
                self.metadata
                    .prefers_headers
                    .store(true, atomic::Ordering::Relaxed);
                Ok(NegotiationUpdate::Updated(self))
            }
            NetworkMessage::SendCmpct(_) => {
                if self.metadata.lowest_common_version < ProtocolVersion::SHORT_IDS_BLOCKS_VERSION {
                    return Err(NegotiationError::UnexpectedMessage(
                        network_message.command(),
                    ));
                }
                self.metadata
                    .prefers_cmpct
                    .store(true, atomic::Ordering::Relaxed);
                Ok(NegotiationUpdate::Updated(self))
            }
            e => Err(NegotiationError::UnexpectedMessage(e.command())),
        }
    }
}

#[derive(Debug)]
enum NegotiationUpdate {
    Finished(CompletedHandshake),
    Updated(InitializedHandshake),
}

#[derive(Debug)]
struct CompletedHandshake {
    metadata: ConnectionMetadata,
    suggested_messages: Vec<NetworkMessage>,
}

impl CompletedHandshake {
    fn take_suggested(&mut self) -> impl Iterator<Item = NetworkMessage> {
        core::mem::take(&mut self.suggested_messages).into_iter()
    }
}

// Errors

#[derive(Debug, Clone)]
enum VersionError {
    IrrelevantMessage(CommandString),
    ConnectionToSelf,
    TooLowVersion(ProtocolVersion),
    NotEnoughServices(ServiceFlags),
}

impl Display for VersionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ConnectionToSelf => write!(f, "connected to self."),
            Self::TooLowVersion(_) => write!(f, "remote version too low."),
            Self::NotEnoughServices(s) => write!(f, "not enough services: {s}"),
            Self::IrrelevantMessage(cmd) => write!(f, "unexpected message: {cmd}"),
        }
    }
}

impl std::error::Error for VersionError {}

#[derive(Debug, Clone)]
enum NegotiationError {
    UnexpectedMessage(CommandString),
}

impl Display for NegotiationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnexpectedMessage(c) => write!(f, "unexpected negotiation message {c}"),
        }
    }
}

impl std::error::Error for NegotiationError {}

#[cfg(test)]
mod tests {
    use p2p::{
        message::NetworkMessage,
        message_network::{UserAgent, VersionMessage},
        ProtocolVersion, ServiceFlags,
    };

    use super::{ConnectionConfig, NegotiationUpdate, Origin, AGENT_VERSION, CLIENT_NAME};

    #[test]
    fn test_complete_successful() {
        let connection = ConnectionConfig::new();

        let version = VersionMessage {
            version: ProtocolVersion::WTXID_RELAY_VERSION,
            services: ServiceFlags::NONE,
            timestamp: 1232132131,
            sender: p2p::Address {
                services: ServiceFlags::NONE,
                address: [0; 8],
                port: 0,
            },
            receiver: p2p::Address {
                services: ServiceFlags::NONE,
                address: [0; 8],
                port: 0,
            },
            start_height: 0,
            nonce: 67,
            user_agent: UserAgent::new(CLIENT_NAME, AGENT_VERSION),
            relay: false,
        };
        let version_message = NetworkMessage::Version(version);
        let mut initial_handshake = connection
            .start_handshake(version_message, Origin::OutBound)
            .unwrap();
        let outbound_messages = initial_handshake.take_suggested();
        for message in outbound_messages {
            assert!(!matches!(message, NetworkMessage::Alert(_)));
        }
        let update = initial_handshake
            .negotiate(NetworkMessage::WtxidRelay)
            .unwrap();
        match update {
            NegotiationUpdate::Updated(handshake) => initial_handshake = handshake,
            NegotiationUpdate::Finished(_) => panic!("handshake incomplete"),
        }
        let update = initial_handshake
            .negotiate(NetworkMessage::SendAddrV2)
            .unwrap();
        match update {
            NegotiationUpdate::Updated(handshake) => initial_handshake = handshake,
            NegotiationUpdate::Finished(_) => panic!("handshake incomplete"),
        }
        let update = initial_handshake
            .negotiate(NetworkMessage::SendHeaders)
            .unwrap();
        match update {
            NegotiationUpdate::Updated(handshake) => initial_handshake = handshake,
            NegotiationUpdate::Finished(_) => panic!("handshake incomplete"),
        }
        let update = initial_handshake.negotiate(NetworkMessage::Verack).unwrap();
        match update {
            NegotiationUpdate::Updated(_) => panic!("handshake is over"),
            NegotiationUpdate::Finished(c) => {
                assert!(c
                    .metadata
                    .prefers_wtxid
                    .load(std::sync::atomic::Ordering::Relaxed));
                assert!(c
                    .metadata
                    .prefers_headers
                    .load(std::sync::atomic::Ordering::Relaxed));
                assert!(c
                    .metadata
                    .prefers_addrv2
                    .load(std::sync::atomic::Ordering::Relaxed));
            }
        }
    }

    #[test]
    fn test_finds_connection_to_self() {
        let connection = ConnectionConfig::new();

        let our_version = VersionMessage {
            version: ProtocolVersion::WTXID_RELAY_VERSION,
            services: ServiceFlags::NONE,
            timestamp: 1232132131,
            sender: p2p::Address {
                services: ServiceFlags::NONE,
                address: [0; 8],
                port: 0,
            },
            receiver: p2p::Address {
                services: ServiceFlags::NONE,
                address: [0; 8],
                port: 0,
            },
            start_height: 0,
            nonce: 67,
            user_agent: UserAgent::new(CLIENT_NAME, AGENT_VERSION),
            relay: false,
        };
        let version_message = NetworkMessage::Version(our_version.clone());
        let initial_handshake = connection.start_handshake(
            version_message,
            Origin::Inbound {
                nonce: 67,
                version: our_version,
            },
        );
        assert!(initial_handshake.is_err());
    }
}
