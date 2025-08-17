use std::{
    fs::File,
    path::Path,
    sync::{mpsc::channel, Arc, Mutex},
    time::Instant,
};

use bitcoin::Network;
use hintfile::Hints;
use kernel::{ChainType, ChainstateManager, ChainstateManagerOptions, ContextBuilder};

use node::{
    bootstrap_dns, elapsed_time, get_blocks_for_range, hashes_from_chain, sync_block_headers,
    AccumulatorState,
};

const CHAIN_TYPE: ChainType = ChainType::SIGNET;
const NETWORK: Network = Network::Signet;
const TASKS: usize = 256;
const BLOCK_FILE_PATH: &str = "./blockfiles";

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
    let block_file_path = Path::new(BLOCK_FILE_PATH);
    std::fs::create_dir(block_file_path).expect("could not create block file directory");
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
    let chainman = ChainstateManager::new(options).unwrap();
    elapsed_time(kernel_start_time);
    let tip = chainman.best_header().height();
    tracing::info!("Kernel best header: {tip}");
    let chain = Arc::new(chainman);
    sync_block_headers(stop_hash, &peers, Arc::clone(&chain), NETWORK);
    tracing::info!("Assume valid height: {}", chain.best_header().height());
    let (tx, rx) = channel();
    let main_routine_time = Instant::now();
    let mut accumulator_state = AccumulatorState::new(rx);
    let acc_task = std::thread::spawn(move || accumulator_state.verify());
    let peers = Arc::new(Mutex::new(peers));
    let mut tasks = Vec::new();
    let chunk_size = chain.best_header().height() as usize / TASKS;
    let hashes = hashes_from_chain(Arc::clone(&chain), chunk_size);
    for (task_id, chunk) in hashes.into_iter().enumerate() {
        let chain = Arc::clone(&chain);
        let tx = tx.clone();
        let peers = Arc::clone(&peers);
        let hints = Arc::clone(&hints);
        let block_task = std::thread::spawn(move || {
            get_blocks_for_range(
                task_id as u32,
                NETWORK,
                block_file_path,
                chain,
                &hints,
                peers,
                tx,
                chunk,
            )
        });
        tasks.push(block_task);
    }
    for task in tasks {
        if let Err(e) = task.join() {
            tracing::warn!("{:?}", e.downcast::<String>());
        }
    }
    drop(tx);
    let acc_result = acc_task.join().unwrap();
    tracing::info!("Verified: {acc_result}");
    elapsed_time(main_routine_time);
}
