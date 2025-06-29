use std::{
    collections::HashSet,
    net::IpAddr,
    sync::Arc,
    time::{Duration, Instant},
};

use accumulator::Accumulator;
use bitcoin::{
    secp256k1::rand::{seq::IteratorRandom, thread_rng},
    BlockHash, Network,
};
#[allow(unused)]
use bitcoin::{OutPoint, Txid};
use loader::{get_block_hashes_from_store, update_acc_from_outpoint_set};

use peers::{
    dns::{DnsQuery, TokioDnsExt},
    PortExt, SeedsExt,
};
use tokio::{
    select,
    sync::{mpsc, Mutex},
    task::JoinHandle,
    time::MissedTickBehavior,
};
use worker::fetch_blocks;

mod loader;
mod worker;

pub const NETWORK: Network = Network::Signet;
const SNAPSHOT_HEIGHT: u32 = 160_000;
// Signet
const ASSUME_VALID_HASH: &str = "0000003ca3c99aff040f2563c2ad8f8ec88bd0fd6b8f0895cfaf1ef90353a62c";
// Bitcoin
// const ASSSUME_VALID_HASH: &str = "000000000000000000010b17283c3c400507969a9c2afd1dcf2082ec5cca2880";
const WORKERS: usize = 16;
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
    tracing::info!("Loading OutPoint set and updating the accumulator");
    let utxo_mutex = Arc::clone(&acc);
    let utxo_handle =
        tokio::task::spawn_blocking(move || update_acc_from_outpoint_set(path, utxo_mutex));
    let path = match NETWORK {
        Network::Signet => "../contrib/signet_headers.sqlite",
        Network::Bitcoin => "../contrib/bitcoin_headers.sqlite",
        _ => unimplemented!("unsupported network"),
    };
    let hashes = get_block_hashes_from_store(path, ASSUME_VALID_HASH.parse::<BlockHash>().unwrap());
    let hashes = hashes
        .into_iter()
        .filter(|(height, _)| height.ne(&0))
        .map(|(_, hash)| hash)
        .collect::<HashSet<BlockHash>>();
    let now = Instant::now();
    let mut rng = thread_rng();
    let peer_lock = peers.lock().await;
    let blocks_per_worker = (SNAPSHOT_HEIGHT as usize + 1) / WORKERS;
    let mut workers = Vec::new();
    let mut worker_id: usize = 0;
    let (done, mut rx) = mpsc::channel(WORKERS);
    tracing::info!("Ready to spawn workers");
    let mut buf = HashSet::new();
    for hash in hashes {
        buf.insert(hash);
        if buf.len() == blocks_per_worker {
            let assigned_hashes = Arc::new(Mutex::new(core::mem::take(&mut buf)));
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
            worker_id += 1;
            buf.clear();
        }
    }
    if !buf.is_empty() {
        let assigned_hashes = Arc::new(Mutex::new(core::mem::take(&mut buf)));
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
    }
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
    let done = now.elapsed().as_secs();
    tracing::info!("Block download complete in {done} seconds");
    tracing::info!("Verified: {}", acc.lock().await.is_zero());
}
