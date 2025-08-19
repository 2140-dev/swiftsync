use std::{
    collections::HashSet,
    fs::File,
    io::Write,
    net::SocketAddr,
    path::Path,
    sync::{
        mpsc::{Receiver, Sender},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

use accumulator::{Accumulator, AccumulatorUpdate};
use bitcoin::{
    consensus,
    key::rand::{seq::SliceRandom, thread_rng},
    script::ScriptExt,
    transaction::TransactionExt,
    BlockHash, Network, OutPoint,
};
use hintfile::Hints;
use kernel::{ChainType, ChainstateManager};
use network::dns::DnsQuery;
use p2p::{
    handshake::ConnectionConfig,
    net::{ConnectionExt, TimeoutParams},
    p2p::{
        message::{InventoryPayload, NetworkMessage},
        message_blockdata::{GetHeadersMessage, Inventory},
        NetworkExt, ProtocolVersion, ServiceFlags,
    },
    SeedsExt, TimedMessage,
};

const PROTOCOL_VERSION: ProtocolVersion = ProtocolVersion::WTXID_RELAY_VERSION;
const MAX_GETDATA: usize = 50_000;

pub fn elapsed_time(then: Instant) {
    let duration_sec = then.elapsed().as_secs_f64();
    tracing::info!("Elapsed time {duration_sec} seconds");
}

#[derive(Debug)]
pub struct AccumulatorState {
    acc: Accumulator,
    update_rx: Receiver<AccumulatorUpdate>,
}

impl AccumulatorState {
    pub fn new(rx: Receiver<AccumulatorUpdate>) -> Self {
        Self {
            acc: Accumulator::new(),
            update_rx: rx,
        }
    }

    pub fn verify(&mut self) -> bool {
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
        .map(|host| SocketAddr::new(host, network.default_p2p_port()))
        .collect()
}

pub fn sync_block_headers(
    stop_hash: BlockHash,
    hosts: &[SocketAddr],
    chainman: Arc<ChainstateManager>,
    network: Network,
    mut timeout_params: TimeoutParams,
) {
    let mut rng = thread_rng();
    let then = Instant::now();
    tracing::info!("Syncing block headers to assume valid hash");
    timeout_params.ping_interval(Duration::from_secs(30));
    loop {
        let random = hosts
            .choose(&mut rng)
            .copied()
            .expect("dns must return at least one peer");
        tracing::info!("Attempting connection to {random}");
        let conn = ConnectionConfig::new()
            .change_network(network)
            .decrease_version_requirement(ProtocolVersion::BIP0031_VERSION)
            .open_connection(random, timeout_params);
        let (writer, mut reader, metrics) = match conn {
            Ok((writer, reader, metrics)) => (writer, reader, metrics),
            Err(_) => continue,
        };
        tracing::info!("Connection established");
        let mut get_next = true;
        loop {
            let mut kill = false;
            if get_next {
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
            }
            while let Ok(Some(message)) = reader.read_message() {
                match message {
                    NetworkMessage::Headers(message) => {
                        for header in message.0 {
                            chainman
                                .process_new_block_headers(&consensus::serialize(&header), true)
                                .expect("process headers failed");
                            get_next = true;
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
                    NetworkMessage::Inv(_) => {
                        kill = true;
                        break;
                    }
                    NetworkMessage::Ping(nonce) => {
                        get_next = false;
                        let _ = writer.send_message(NetworkMessage::Pong(nonce));
                    }
                    e => {
                        get_next = false;
                        tracing::info!("Ignoring message {}", e.command());
                    }
                }
            }
            if kill {
                break;
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn get_blocks_for_range(
    task_id: u32,
    timeout_params: TimeoutParams,
    blocks_per_sec: f64,
    _ping_timeout: Duration,
    network: Network,
    block_dir: &Path,
    chain: Arc<ChainstateManager>,
    hints: &Hints,
    peers: Arc<Mutex<Vec<SocketAddr>>>,
    updater: Sender<AccumulatorUpdate>,
    mut batch: Vec<BlockHash>,
) {
    tracing::info!("{task_id} assigned {} blocks", batch.len());
    let mut rng = thread_rng();
    loop {
        let peer = {
            let lock_opt = peers.lock().ok();
            let socket_addr = lock_opt.and_then(|lock| lock.choose(&mut rng).copied());
            socket_addr
        };
        let Some(peer) = peer else { continue };
        // tracing::info!("Connecting to {peer}");
        let conn = ConnectionConfig::new()
            .change_network(network)
            .request_addr()
            .set_service_requirement(ServiceFlags::NETWORK)
            .decrease_version_requirement(ProtocolVersion::BIP0031_VERSION)
            .open_connection(peer, timeout_params);
        let Ok((writer, mut reader, metrics)) = conn else {
            // tracing::warn!("Connection failed");
            continue;
        };
        // tracing::info!("Connection successful");
        let payload = InventoryPayload(batch.iter().map(|hash| Inventory::Block(*hash)).collect());
        // tracing::info!("Requesting {} blocks", payload.0.len());
        let getdata = NetworkMessage::GetData(payload);
        if writer.send_message(getdata).is_err() {
            continue;
        }
        while let Ok(Some(message)) = reader.read_message() {
            match message {
                NetworkMessage::Ping(nonce) => {
                    let _ = writer.send_message(NetworkMessage::Pong(nonce));
                }
                NetworkMessage::Block(block) => {
                    let hash = block.block_hash();
                    batch.retain(|b| hash.ne(b));
                    let kernal_hash: kernel::BlockHash = kernel::BlockHash {
                        hash: hash.to_byte_array(),
                    };
                    let block_index = chain
                        .block_index_by_hash(kernal_hash)
                        .expect("header is in best chain.");
                    let block_height = block_index.height().unsigned_abs();
                    let unspent_indexes: HashSet<u64> =
                        hints.get_block_offsets(block_height).into_iter().collect();
                    // tracing::info!("{task_id} -> {block_height}:{hash}");
                    let file_path = block_dir.join(format!("{hash}.block"));
                    let file = File::create_new(file_path);
                    let mut file = match file {
                        Ok(file) => file,
                        Err(e) => {
                            tracing::warn!("Conflicting open files at: {}", block_height);
                            tracing::warn!("{e}");
                            panic!("files cannot conflict");
                        }
                    };
                    let block_bytes = consensus::serialize(&block);
                    file.write_all(&block_bytes)
                        .expect("failed to write block file");
                    // tracing::info!("Wrote {hash} to file");
                    let (_, transactions) = block.into_parts();
                    let mut output_index = 0;
                    for transaction in transactions {
                        let tx_hash = transaction.compute_txid();
                        if !transaction.is_coinbase() {
                            for input in transaction.inputs {
                                let input_hash = accumulator::hash_outpoint(input.previous_output);
                                let update = AccumulatorUpdate::Spent(input_hash);
                                updater
                                    .send(update)
                                    .expect("accumulator task must not panic");
                            }
                        }
                        for (vout, txout) in transaction.outputs.iter().enumerate() {
                            if !txout.script_pubkey.is_op_return()
                                && !txout.script_pubkey.len() > 10_000
                                && !unspent_indexes.contains(&output_index)
                            {
                                let outpoint = OutPoint {
                                    txid: tx_hash,
                                    vout: vout as u32,
                                };
                                let input_hash = accumulator::hash_outpoint(outpoint);
                                let update = AccumulatorUpdate::Add(input_hash);
                                updater
                                    .send(update)
                                    .expect("accumulator task must not panic");
                            }
                            output_index += 1
                        }
                    }
                    if batch.len() % 100 == 0 {
                        tracing::info!("{task_id} has {} remaining blocks", batch.len());
                    }
                    if batch.is_empty() {
                        tracing::info!("All block ranges fetched: {task_id}");
                        return;
                    }
                }
                NetworkMessage::AddrV2(payload) => {
                    if let Ok(mut lock) = peers.lock() {
                        let addrs: Vec<SocketAddr> = payload
                            .0
                            .into_iter()
                            .filter_map(|addr| {
                                addr.socket_addr().ok().map(|sock| (addr.port, sock))
                            })
                            .map(|(_, addr)| addr)
                            .collect();
                        // tracing::info!("Adding {} peers", addrs.len());
                        lock.extend(addrs);
                    }
                }
                _ => (),
            }
            if let Some(message_rate) = metrics.message_rate(TimedMessage::Block) {
                if message_rate.total_count() < 100 {
                    continue;
                }
                let Some(rate) = message_rate.messages_per_secs(Instant::now()) else {
                    continue;
                };
                if rate < blocks_per_sec {
                    tracing::warn!("Disconnecting from {task_id} for stalling");
                    break;
                }
            }
            // if metrics.ping_timed_out(ping_timeout) {
            // tracing::warn!("{task_id} failed to respond to a ping");
            // break;
            // }
        }
        if batch.is_empty() {
            break;
        }
    }
    tracing::info!("All block ranges fetched: {task_id}");
}

pub fn hashes_from_chain(chain: Arc<ChainstateManager>, jobs: usize) -> Vec<Vec<BlockHash>> {
    let height = chain.best_header().height();
    let mut hashes = Vec::with_capacity(height as usize);
    let mut curr = chain.best_header();
    let tip_hash = BlockHash::from_byte_array(curr.block_hash().hash);
    hashes.push(tip_hash);
    let mut out = Vec::new();
    while let Ok(next) = curr.prev() {
        if next.height() == 0 {
            break;
        }
        let hash = BlockHash::from_byte_array(next.block_hash().hash);
        hashes.push(hash);
        curr = next;
    }
    // These blocks are empty. Fetch the maximum amount of blocks.
    let first_epoch = hashes.split_off(hashes.len() - 200_000);
    let first_chunks: Vec<Vec<BlockHash>> = first_epoch
        .chunks(MAX_GETDATA)
        .map(|slice| slice.to_vec())
        .collect();
    out.extend(first_chunks);
    // These start to get larger, but are still small
    let next_epoch = hashes.split_off(hashes.len() - 100_000);
    let next_chunks: Vec<Vec<BlockHash>> = next_epoch
        .chunks(MAX_GETDATA / 2)
        .map(|slice| slice.to_vec())
        .collect();
    out.extend(next_chunks);
    // Still not entirely full, but almost there
    let to_segwit = hashes.split_off(hashes.len() - 100_000);
    let to_segwit_chunks: Vec<Vec<BlockHash>> = to_segwit
        .chunks(MAX_GETDATA / 4)
        .map(|slice| slice.to_vec())
        .collect();
    out.extend(to_segwit_chunks);
    // Now divide the rest among jobs
    let chunk_size = hashes.len() / jobs;
    let rest: Vec<Vec<BlockHash>> = hashes
        .chunks(chunk_size)
        .map(|slice| slice.to_vec())
        .collect();
    out.extend(rest);
    out
}

pub trait ChainExt {
    fn chain_type(&self) -> ChainType;
}

impl ChainExt for Network {
    fn chain_type(&self) -> ChainType {
        match self {
            Network::Bitcoin => ChainType::MAINNET,
            Network::Signet => ChainType::SIGNET,
            _ => unimplemented!("choose bitcoin or signet"),
        }
    }
}
