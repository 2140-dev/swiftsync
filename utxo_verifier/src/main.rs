use std::{
    collections::HashSet,
    net::IpAddr,
    sync::Arc,
    time::{Duration, Instant},
};

use accumulator::Accumulator;
use bitcoin::{
    BlockHash, Network, block,
    p2p::{message::NetworkMessage, message_blockdata::GetHeadersMessage},
    secp256k1::rand::{seq::IteratorRandom, thread_rng},
};
#[allow(unused)]
use bitcoin::{OutPoint, Txid};
use headers::{AcceptHeaderChanges, BlockTree};
use loader::update_acc_from_outpoint_set;
use p2p::{
    ConnectionBuilder, ConnectionContext,
    tokio_ext::{
        ReadError, TokioConnectionExt, TokioReadNetworkMessageExt, TokioWriteNetworkMessageExt,
    },
};
use peers::{
    PortExt, SeedsExt,
    dns::{DnsQuery, TokioDnsExt},
};
use tokio::{
    net::TcpStream,
    select,
    sync::{Mutex, mpsc, oneshot},
    task::JoinHandle,
    time::{MissedTickBehavior, timeout},
};
use worker::fetch_blocks;

mod headers;
mod loader;
mod worker;

pub const NETWORK: Network = Network::Signet;
const SNAPSHOT_HEIGHT: u32 = 160_000;
const WORKERS: usize = 1;
#[allow(unused)]
const DUP_COINBASE_ONE: &str = "e3bf3d07d4b0375638d5f1db5255fe07ba2c4cb067cd81b84ee974b6585fb468";
#[allow(unused)]
const DUP_COINBASE_TWO: &str = "d5d27987d2a3dfc724e359870c6644b40e497bdc0589a033220fe15429d88599";

async fn bootstrap(peers: Arc<Mutex<HashSet<IpAddr>>>) {
    let seeds = NETWORK.dns_seeds();
    for seed in seeds {
        let query = DnsQuery::new_cloudflare(seed).lookup_async().await;
        if let Ok(addrs) = query {
            tracing::info!("Adding IPs {} from {}", addrs.len(), seed);
            let mut table = peers.lock().await;
            table.extend(addrs);
        }
    }
    tracing::info!("DNS task exit");
}

async fn sync_header_chain(
    peers: Arc<Mutex<HashSet<IpAddr>>>,
    chain: Arc<Mutex<BlockTree>>,
    done: oneshot::Sender<()>,
) {
    let expected_port = NETWORK.port();
    loop {
        let lock = peers.lock().await;
        let peer = lock.iter().choose(&mut thread_rng()).copied();
        drop(lock);
        match peer {
            Some(ip_addr) => {
                let connection_result = ConnectionBuilder::new()
                    .no_cmpct_blocks()
                    .announce_by_inv()
                    .add_start_height(0)
                    .change_network(NETWORK)
                    .open_connection((ip_addr, expected_port))
                    .await;
                tracing::info!("Successful connection to peer {}", ip_addr);
                if let Ok((stream, ctx)) = connection_result {
                    request_headers_from_connection(stream, ctx, Arc::clone(&chain)).await;
                    let chain = chain.lock().await;
                    if chain.height().eq(&SNAPSHOT_HEIGHT) {
                        tracing::info!("Header sync task exit");
                        done.send(()).unwrap();
                        return;
                    }
                }
            }
            None => {
                tracing::info!("No peer available for header sync. Waiting a few seconds.");
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
    }
}

async fn request_headers_from_connection(
    mut stream: TcpStream,
    mut ctx: ConnectionContext,
    chain: Arc<Mutex<BlockTree>>,
) {
    let mut chain = chain.lock().await;
    loop {
        let stop_hash = BlockHash::from_byte_array([0u8; 32]);
        let request = GetHeadersMessage {
            version: 0x00,
            locator_hashes: vec![chain.tip_hash()],
            stop_hash,
        };
        tracing::info!("Header chain height: {}", chain.height());
        let message = NetworkMessage::GetHeaders(request);
        if let Err(e) = stream.write_message(message, &mut ctx).await {
            tracing::warn!("Header sync peer failed: {e}");
        }
        let timeout = timeout(
            Duration::from_secs(5),
            wait_for_header_response(&mut stream, &mut ctx),
        )
        .await;
        match timeout {
            Ok(responded) => match responded {
                Ok(headers) => {
                    for header in headers {
                        let apply_header = chain.accept_header(header);
                        match apply_header {
                            AcceptHeaderChanges::Accepted { connected_at } => {
                                if connected_at.height.eq(&SNAPSHOT_HEIGHT) {
                                    tracing::info!("Headers synced to snapshot height");
                                    return;
                                }
                            }
                            _ => return,
                        }
                    }
                }
                Err(r) => {
                    tracing::info!("Header sync peer encountered an error: {r}");
                    return;
                }
            },
            Err(_) => {
                tracing::info!("Header sync peer timed out");
                return;
            }
        }
    }
}

async fn wait_for_header_response(
    stream: &mut TcpStream,
    ctx: &mut ConnectionContext,
) -> Result<Vec<block::Header>, ReadError> {
    loop {
        let data = stream.read_message(ctx).await?;
        if let Some(data) = data {
            match data {
                NetworkMessage::Ping(nonce) => {
                    tracing::info!("Ping {nonce}");
                    let _ = stream.write_message(NetworkMessage::Pong(nonce), ctx).await;
                }
                NetworkMessage::Headers(headers) => {
                    return Ok(headers);
                }
                other => tracing::info!("{}", other.cmd()),
            }
        }
    }
}

#[derive(Debug)]
struct Worker {
    worker_id: usize,
    hashes: Arc<Mutex<HashSet<BlockHash>>>,
    task: JoinHandle<()>,
}

async fn check_workers(workers: &[Worker]) -> bool {
    let mut all_empty = true;
    for worker in workers.iter() {
        let lock = worker.hashes.lock().await;
        if !lock.is_empty() {
            all_empty = false;
        }
    }
    all_empty
}

#[tokio::main]
async fn main() {
    let subscriber = tracing_subscriber::FmtSubscriber::new();
    tracing::subscriber::set_global_default(subscriber).unwrap();
    let mut args = std::env::args();
    let _ = args.next();
    let path = args
        .next()
        .expect("Provide a file path to the utxos.sqlite/outpoints.sqlite file");
    #[allow(unused_mut)]
    let mut acc = Accumulator::new();
    // These outpoints will show up twice, but can only be spent once
    //
    //let coinbase_one = DUP_COINBASE_ONE.parse::<Txid>().unwrap();
    // let coinbase_two = DUP_COINBASE_TWO.parse::<Txid>().unwrap();
    // acc.spend(OutPoint {
    // txid: coinbase_one,
    //  vout: 0,
    // });
    // acc.spend(OutPoint {
    // txid: coinbase_two,
    // vout: 0,
    // });
    let acc = Arc::new(Mutex::new(acc));
    let peers = Arc::new(Mutex::new(HashSet::<IpAddr>::new()));
    tracing::info!("Starting DNS thread");
    let dns_mutex = Arc::clone(&peers);
    tokio::task::spawn(async move { bootstrap(dns_mutex).await });
    let chain = Arc::new(Mutex::new(BlockTree::from_genesis(NETWORK)));
    let header_chain_mutex = Arc::clone(&chain);
    let header_peer_mutex = Arc::clone(&peers);
    let (tx, rx) = oneshot::channel();
    tracing::info!("Background syncing header chain");
    tokio::task::spawn(async move {
        sync_header_chain(header_peer_mutex, header_chain_mutex, tx).await
    });
    tracing::info!("Loading OutPoint set and updating the accumulator");
    let utxo_mutex = Arc::clone(&acc);
    let utxo_handle =
        tokio::task::spawn_blocking(move || update_acc_from_outpoint_set(path, utxo_mutex));
    let _ = rx.await;
    tracing::info!("Ready to spawn workers");
    let now = Instant::now();
    let chain_lock = chain.lock().await;
    let mut rng = thread_rng();
    let peer_lock = peers.lock().await;
    let blocks_per_worker = (chain_lock.height() as usize + 1) / WORKERS;
    let mut workers = Vec::new();
    let mut worker_id: usize = 0;
    let (done, mut rx) = mpsc::channel(WORKERS);
    let assume_valid_hash = "0000003ca3c99aff040f2563c2ad8f8ec88bd0fd6b8f0895cfaf1ef90353a62c"
        .parse::<BlockHash>()
        .unwrap();
    assert_eq!(chain_lock.tip_hash(), assume_valid_hash);
    let hashes = chain_lock
        .iter_headers()
        .filter(|indexed| indexed.height.ne(&0))
        .map(|indexed| indexed.header.block_hash())
        .collect();
    let assigned_hashes = Arc::new(Mutex::new(hashes));
    let hashes_for_worker = Arc::clone(&assigned_hashes);
    let acc_state = Arc::clone(&acc);
    let peer = peer_lock.iter().choose(&mut rng).copied().unwrap();
    let port = NETWORK.port();
    let signal = done.clone();
    tracing::info!("Spawning worker {worker_id}");
    let handle = tokio::task::spawn(async move {
        fetch_blocks(
            acc_state,
            hashes_for_worker,
            (peer, port),
            signal,
            worker_id,
        )
        .await;
    });
    let worker = Worker {
        worker_id,
        hashes: assigned_hashes,
        task: handle,
    };
    workers.push(worker);
    tracing::info!("All tasks spawned");
    let mut interval = tokio::time::interval(Duration::from_millis(250));
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        select! {
            done = rx.recv() => {
                if let Some(done) = done {
                    tracing::info!("Worker {done} is done");
                    if check_workers(&workers).await {
                        tracing::info!("All workers are done");
                        break;
                    }
                }
            },
            _ = interval.tick() => {
                for worker in &mut workers {
                    // Redeploy failed tasks
                    if worker.task.is_finished() {
                        let hashes = worker.hashes.lock().await;
                        if !hashes.is_empty() {
                            tracing::info!("Redeploying {}", worker.worker_id);
                            let assigned_hashes = Arc::clone(&worker.hashes);
                            let acc_state = Arc::clone(&acc);
                            let peer = peer_lock.iter().choose(&mut rng).copied().unwrap();
                            let port = NETWORK.port();
                            let done = done.clone();
                            let worker_id = worker.worker_id;
                            let handle = tokio::task::spawn(async move {
                                fetch_blocks(acc_state, assigned_hashes, (peer, port), done, worker_id).await;
                            });
                            worker.task = handle;
                        }
                    }
                }
            }
        }
    }
    let _ = utxo_handle.await;
    let done = now.elapsed().as_secs() / 60;
    tracing::info!("Block download complete it {done} minutes");
    tracing::info!("Verified: {}", acc.lock().await.is_zero());
}
