use std::{
    collections::HashSet,
    net::IpAddr,
    sync::{Arc, mpsc::channel},
    time::Instant,
};

use bitcoin::{BlockHash, Network};

#[allow(unused)]
use bitcoin::{OutPoint, Txid};
use hasher::update_accumulator_from_blocks;
use job::{divide_jobs, fetch_blocks};
use loader::{get_block_hashes_from_store, update_acc_from_outpoint_set};

use peers::{SeedsExt, dns::DnsQuery};

mod hasher;
mod job;
mod loader;

pub const NETWORK: Network = Network::Signet;
// Signet
const ASSUME_VALID_HASH: &str = "0000003ca3c99aff040f2563c2ad8f8ec88bd0fd6b8f0895cfaf1ef90353a62c";
// Bitcoin
// const ASSUME_VALID_HASH: &str = "000000000000000000010b17283c3c400507969a9c2afd1dcf2082ec5cca2880";
const WORKERS: usize = 32;
pub const DUP_COINBASE_ONE: &str =
    "e3bf3d07d4b0375638d5f1db5255fe07ba2c4cb067cd81b84ee974b6585fb468";
pub const DUP_COINBASE_TWO: &str =
    "d5d27987d2a3dfc724e359870c6644b40e497bdc0589a033220fe15429d88599";
const REQUEST_SIZE: usize = 2_000;

fn bootstrap() -> HashSet<IpAddr> {
    let seeds = NETWORK.dns_seeds();
    let mut peers = HashSet::with_capacity(300);
    for seed in seeds {
        let query = DnsQuery::new_cloudflare(seed).lookup();
        if let Ok(addrs) = query {
            tracing::info!("Adding IPs {} from {}", addrs.len(), seed);
            peers.extend(addrs);
        }
    }
    tracing::info!("DNS task exit");
    peers
}

#[derive(Debug, Clone, Copy)]
enum AccumulatorUpdate {
    Spent([u8; 32]),
    Created([u8; 32]),
}

fn main() {
    let subscriber = tracing_subscriber::FmtSubscriber::new();
    tracing::subscriber::set_global_default(subscriber).unwrap();
    let mut args = std::env::args();
    let _ = args.next();
    let path = args
        .next()
        .expect("Provide a file path to the utxos.sqlite/outpoints.sqlite file");
    tracing::info!("Fetching peers from DNS");
    let peers = Arc::new(bootstrap());
    tracing::info!("Loading OutPoint set and updating the accumulator");
    let utxo_handle = std::thread::spawn(move || update_acc_from_outpoint_set(path));
    let path = match NETWORK {
        Network::Signet => "../contrib/signet_headers.sqlite",
        Network::Bitcoin => "../contrib/bitcoin_headers.sqlite",
        _ => unimplemented!("unsupported network"),
    };
    let hashes = get_block_hashes_from_store(path, ASSUME_VALID_HASH.parse::<BlockHash>().unwrap());
    let hashes = hashes.into_values().collect::<Vec<BlockHash>>();
    let (tx, rx) = channel();
    let block_handle = std::thread::spawn(move || update_accumulator_from_blocks(rx));
    let now = Instant::now();
    tracing::info!("Spawning workers");
    let mut handles = Vec::with_capacity(WORKERS);
    let jobs = divide_jobs(hashes, WORKERS);
    for (id, job) in jobs.into_iter().enumerate() {
        let windows = job
            .chunks(REQUEST_SIZE)
            .map(|slice| slice.to_vec())
            .collect();
        let sender = tx.clone();
        let peers = Arc::clone(&peers);
        let handle = std::thread::spawn(move || fetch_blocks(id, sender, windows, peers.clone()));
        handles.push(handle);
    }
    tracing::info!("Waiting for jobs");
    for handle in handles {
        handle.join().unwrap();
    }
    drop(tx);
    let utxo_acc = utxo_handle.join().unwrap();
    let block_acc = block_handle.join().unwrap();
    let done = now.elapsed().as_secs();
    tracing::info!("Block download complete in {done} seconds");
    assert_eq!(utxo_acc, block_acc);
    tracing::info!("Verified");
}
