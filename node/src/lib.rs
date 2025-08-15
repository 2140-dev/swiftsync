use std::{
    net::SocketAddr,
    sync::{mpsc::Receiver, Arc},
    time::Instant,
};

use accumulator::{Accumulator, AccumulatorUpdate};
use bitcoin::{
    consensus,
    key::rand::{seq::SliceRandom, thread_rng},
    BlockHash, Network,
};
use kernel::ChainstateManager;
use network::dns::DnsQuery;
use p2p::{
    handshake::ConnectionConfig,
    net::ConnectionExt,
    p2p::{message::NetworkMessage, message_blockdata::GetHeadersMessage, ProtocolVersion},
    SeedsExt,
};

const PROTOCOL_VERSION: ProtocolVersion = ProtocolVersion::WTXID_RELAY_VERSION;

pub fn elapsed_time(then: Instant) {
    let duration_sec = then.elapsed().as_secs_f64();
    tracing::info!("Elapsed time {duration_sec} seconds");
}

pub trait PortExt {
    fn port(&self) -> u16;
}

impl PortExt for Network {
    fn port(&self) -> u16 {
        match self {
            Network::Signet => 38333,
            Network::Bitcoin => 8333,
            _ => unimplemented!(),
        }
    }
}

#[derive(Debug)]
pub struct AccumulatorState {
    acc: Accumulator,
    update_rx: Receiver<AccumulatorUpdate>,
}

impl AccumulatorState {
    fn new(rx: Receiver<AccumulatorUpdate>) -> Self {
        Self {
            acc: Accumulator::new(),
            update_rx: rx,
        }
    }

    fn verify(&mut self) -> bool {
        while let Ok(update) = self.update_rx.recv() {
            self.acc.update(update);
        }
        self.acc.is_zero()
    }
}

pub fn bootstrap_dns(network: Network) -> Vec<SocketAddr> {
    let mut all_hosts = Vec::new();
    for seed in network.seeds() {
        let hosts = DnsQuery::new_cloudflare(seed).lookup().unwrap_or_default();
        all_hosts.extend_from_slice(&hosts);
    }
    all_hosts
        .into_iter()
        .map(|host| SocketAddr::new(host, network.port()))
        .collect()
}

pub fn sync_block_headers(
    stop_hash: BlockHash,
    hosts: &[SocketAddr],
    chainman: Arc<ChainstateManager>,
    network: Network,
) {
    let mut rng = thread_rng();
    let then = Instant::now();
    tracing::info!("Syncing block headers to assume valid hash");
    loop {
        let random = hosts
            .choose(&mut rng)
            .copied()
            .expect("dns must return at least one peer");
        tracing::info!("Attempting connection to {random}");
        let conn = ConnectionConfig::new()
            .change_network(network)
            .decrease_version_requirement(ProtocolVersion::BIP0031_VERSION)
            .open_connection(random);
        let (writer, mut reader, metrics) = match conn {
            Ok((writer, reader, metrics)) => (writer, reader, metrics),
            Err(_) => continue,
        };
        tracing::info!("Connection established");
        if writer.send_message(NetworkMessage::GetAddr).is_err() {
            continue;
        }
        loop {
            let curr = chainman.best_header().block_hash().hash;
            let locator = BlockHash::from_byte_array(curr);
            let getheaders = GetHeadersMessage {
                version: PROTOCOL_VERSION,
                locator_hashes: vec![locator],
                stop_hash: BlockHash::GENESIS_PREVIOUS_BLOCK_HASH,
            };
            tracing::info!("Requesting {locator}");
            if writer
                .send_message(NetworkMessage::GetHeaders(getheaders))
                .is_err()
            {
                break;
            }
            while let Ok(Some(message)) = reader.read_message() {
                match message {
                    NetworkMessage::Headers(message) => {
                        for header in message.0 {
                            chainman
                                .process_new_block_headers(&consensus::serialize(&header), true)
                                .expect("process headers failed");
                            if header.block_hash().eq(&stop_hash) {
                                tracing::info!("Done syncing block headers");
                                if let Some(message_rate) =
                                    metrics.message_rate(p2p::TimedMessage::BlockHeaders)
                                {
                                    let mps = message_rate
                                        .messages_per_secs(Instant::now())
                                        .unwrap_or(0.);
                                    tracing::info!("Peer responses per second: {mps}");
                                }
                                elapsed_time(then);
                                return;
                            }
                        }
                        tracing::info!("Update chain tip: {}", chainman.best_header().height());
                        break;
                    }
                    NetworkMessage::Ping(nonce) => {
                        let _ = writer.send_message(NetworkMessage::Pong(nonce));
                    }
                    e => tracing::info!("Ignoring message {}", e.command()),
                }
            }
        }
    }
}
