use std::{
    collections::HashSet,
    fs::File,
    io::Write,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::PathBuf,
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
use p2p::{
    dns::DnsQueryExt,
    handshake::ConnectionConfig,
    net::{ConnectionExt, TimeoutParams},
    p2p::{
        message::{InventoryPayload, NetworkMessage},
        message_blockdata::{GetHeadersMessage, Inventory},
        NetworkExt, ProtocolVersion, ServiceFlags,
    },
    TimedMessage,
};

const PROTOCOL_VERSION: ProtocolVersion = ProtocolVersion::WTXID_RELAY_VERSION;
const CHUNK_SIZE: usize = 100;
const CONSIDERED_DEAD: f64 = 0.1;

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
    let cloudflare = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)), 53);
    network
        .query_dns_seeds(cloudflare)
        .into_iter()
        .map(|ip| SocketAddr::new(ip, network.default_p2p_port()))
        .collect()
}

pub fn sync_block_headers(
    stop_height: u32,
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
        let curr = chainman.best_header();
        let locator = BlockHash::from_byte_array(curr.block_hash().hash);
        let curr_height = curr.height() as u32;
        if curr_height.eq(&stop_height) {
            tracing::info!("Using existing header state");
            return;
        }
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
            continue;
        }
        while let Ok(Some(message)) = reader.read_message() {
            match message {
                NetworkMessage::Headers(message) => {
                    for header in message.0 {
                        chainman
                            .process_new_block_headers(&consensus::serialize(&header), true)
                            .expect("process headers failed");
                        let curr_height = chainman.best_header().height() as u32;
                        if curr_height.eq(&stop_height) {
                            tracing::info!("Done syncing block headers");
                            if let Some(message_rate) =
                                metrics.message_rate(p2p::TimedMessage::BlockHeaders)
                            {
                                let mps =
                                    message_rate.messages_per_secs(Instant::now()).unwrap_or(0.);
                                tracing::info!("Peer responses per second: {mps}");
                            }
                            elapsed_time(then);
                            return;
                        }
                    }
                    tracing::info!("Update chain tip: {}", chainman.best_header().height());
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
                NetworkMessage::Inv(_) => {
                    break;
                }
                NetworkMessage::Ping(nonce) => {
                    let _ = writer.send_message(NetworkMessage::Pong(nonce));
                }
                e => {
                    tracing::info!("Ignoring message {}", e.command());
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn get_blocks_for_range(
    task_id: u32,
    timeout_params: TimeoutParams,
    blocks_per_sec: f64,
    network: Network,
    block_dir: Option<PathBuf>,
    chain: Arc<ChainstateManager>,
    hints: Arc<Mutex<Hints>>,
    peers: Arc<Mutex<Vec<SocketAddr>>>,
    updater: Sender<AccumulatorUpdate>,
    hashes: Arc<Mutex<Vec<Vec<BlockHash>>>>,
) {
    let mut batch = Vec::new();
    let mut rng = thread_rng();
    let stop_height = { hints.lock().unwrap().stop_height() };
    loop {
        let peer = {
            let lock_opt = peers.lock().ok();
            let socket_addr = lock_opt.and_then(|lock| lock.choose(&mut rng).copied());
            socket_addr
        };
        if batch.is_empty() {
            let mut jobs_lock = hashes.lock().expect("could not take lock on hashes");
            let Some(next) = jobs_lock.pop() else {
                return;
            };
            tracing::info!("[thread {task_id:2}]: requesting next batch");
            batch = next;
        }
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
        let mut completed_batches = 0;
        tracing::info!("[thread {task_id:2}]: established connection {peer}");
        let payload = InventoryPayload(batch.iter().map(|hash| Inventory::Block(*hash)).collect());
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
                    // tracing::info!("[thread {task_id:2}]: {hash}");
                    batch.retain(|b| hash.ne(b));
                    let kernal_hash: kernel::BlockHash = kernel::BlockHash {
                        hash: hash.to_byte_array(),
                    };
                    let block_index = chain
                        .block_index_by_hash(kernal_hash)
                        .expect("header is in best chain.");
                    let block_height = block_index.height().unsigned_abs();
                    let unspent_indexes: HashSet<u64> = {
                        let mut hint_ref = hints.lock().unwrap();
                        hint_ref.get_indexes(block_height).into_iter().collect()
                    };
                    if let Some(block_dir) = block_dir.as_ref() {
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
                        file.sync_data().expect("could not sync file with OS");
                    }
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
                    if batch.is_empty() {
                        let mut jobs_lock = hashes.lock().expect("could not take lock on hashes");
                        let Some(next) = jobs_lock.pop() else {
                            tracing::info!("[thread {task_id:2}]: no jobs remaining, please wait for other threads");
                            return;
                        };
                        batch = next;
                        completed_batches += 1;
                        tracing::info!(
                            "[thread {task_id:2}]: blocks downloaded: {}/{}",
                            CHUNK_SIZE * completed_batches,
                            stop_height,
                        );
                        let percent = (1.
                            - ((CHUNK_SIZE * jobs_lock.len()) as f32 / stop_height as f32))
                            * 100.0;
                        tracing::info!(
                            "[thread  m]: progress: {:.6}% ; blocks remaining: {}/{}",
                            percent,
                            CHUNK_SIZE * jobs_lock.len(),
                            stop_height,
                        );
                        let payload = InventoryPayload(
                            batch.iter().map(|hash| Inventory::Block(*hash)).collect(),
                        );
                        let getdata = NetworkMessage::GetData(payload);
                        if writer.send_message(getdata).is_err() {
                            break;
                        }
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
                        lock.extend(addrs);
                    }
                }
                _ => (),
            }
            if let Some(message_rate) = metrics.message_rate(TimedMessage::Block) {
                let Some(rate) = message_rate.messages_per_secs(Instant::now()) else {
                    continue;
                };
                if rate < CONSIDERED_DEAD {
                    tracing::warn!("[thread {task_id:2}]: block rate considered dead");
                    break;
                }
                if rate < blocks_per_sec && message_rate.total_count() > 20 {
                    tracing::warn!("[thread {task_id:2}]: insufficient blocks/second rate");
                    break;
                }
            }
        }
    }
}

pub fn hashes_from_chain(chain: Arc<ChainstateManager>) -> Vec<Vec<BlockHash>> {
    let height = chain.best_header().height();
    let mut hashes = Vec::with_capacity(height as usize);
    let mut curr = chain.best_header();
    let tip_hash = BlockHash::from_byte_array(curr.block_hash().hash);
    hashes.push(tip_hash);
    while let Ok(next) = curr.prev() {
        if next.height() == 0 {
            break;
        }
        let hash = BlockHash::from_byte_array(next.block_hash().hash);
        hashes.push(hash);
        curr = next;
    }
    hashes
        .chunks(CHUNK_SIZE)
        .map(|slice| slice.to_vec())
        .rev()
        .collect()
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
