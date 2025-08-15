use std::{fs::File, sync::Arc, time::Instant};

use bitcoin::Network;
use hintfile::Hints;
use kernel::{ChainType, ChainstateManager, ChainstateManagerOptions, ContextBuilder};

use node::{bootstrap_dns, elapsed_time, sync_block_headers};

const CHAIN_TYPE: ChainType = ChainType::SIGNET;
const NETWORK: Network = Network::Signet;

fn main() {
    let mut args = std::env::args();
    let _ = args.next();
    let hint_path = args.next().expect("Usage: <path_to_hints_file>");
    // Logging
    let subscriber = tracing_subscriber::FmtSubscriber::new();
    tracing::subscriber::set_global_default(subscriber).unwrap();
    let hintfile_start_time = Instant::now();
    tracing::info!("Reading in {hint_path}");
    let mut hintfile = File::open(hint_path).expect("invalid hintfile path");
    let hints = Arc::new(Hints::from_file(&mut hintfile));
    elapsed_time(hintfile_start_time);
    let stop_hash = hints.stop_hash();
    tracing::info!("Assume valid hash: {stop_hash}");
    tracing::info!("Finding peers with DNS");
    let dns_start_time = Instant::now();
    let peers = bootstrap_dns(NETWORK);
    elapsed_time(dns_start_time);
    tracing::info!("Initializing bitcoin kernel");
    let kernel_start_time = Instant::now();
    let ctx = ContextBuilder::new()
        .chain_type(CHAIN_TYPE)
        .build()
        .unwrap();
    let options = ChainstateManagerOptions::new(&ctx, ".", "./blocks").unwrap();
    let _context = Arc::new(ctx);
    let chainman = ChainstateManager::new(options).unwrap();
    elapsed_time(kernel_start_time);
    let tip = chainman.best_header().height();
    tracing::info!("Kernel best header: {tip}");
    let chain = Arc::new(chainman);
    sync_block_headers(stop_hash, &peers, Arc::clone(&chain), NETWORK);
    tracing::info!("Assume valid height: {}", chain.best_header().height());
}
