use std::{
    fs::File,
    path::PathBuf,
    sync::{mpsc::channel, Arc, Mutex},
    time::{Duration, Instant},
};

use bitcoin::{consensus, BlockHash, Network};
use hintfile::Hints;
use kernel::{ChainstateManager, ChainstateManagerOptions, ContextBuilder};

use node::{
    bootstrap_dns, elapsed_time, get_blocks_for_range, hashes_from_chain, sync_block_headers,
    AccumulatorState, ChainExt,
};
use p2p::net::TimeoutParams;

const PING_INTERVAL: Duration = Duration::from_secs(10 * 60);

configure_me::include_config!();

fn main() {
    let (config, _) = Config::including_optional_config_files::<&[&str]>(&[]).unwrap_or_exit();
    let hint_path = config.hintfile;
    let blocks_dir = config.blocks_dir;
    let network = config
        .network
        .parse::<Network>()
        .expect("invalid network string");
    let ping_timeout = Duration::from_secs(config.ping_timeout);
    let block_per_sec = config.min_blocks_per_sec;
    let task_num = config.tasks;
    let tcp_timeout = Duration::from_secs(config.tcp_timeout);
    let read_timeout = Duration::from_secs(config.read_timeout);
    let write_timeout = Duration::from_secs(config.write_timeout);
    let mut timeout_conf = TimeoutParams::new();
    timeout_conf.read_timeout(read_timeout);
    timeout_conf.write_timeout(write_timeout);
    timeout_conf.tcp_handshake_timeout(tcp_timeout);
    timeout_conf.ping_interval(PING_INTERVAL);
    let subscriber = tracing_subscriber::FmtSubscriber::new();
    tracing::subscriber::set_global_default(subscriber).unwrap();
    let hintfile_start_time = Instant::now();
    tracing::info!("Reading in {hint_path}");
    let mut hintfile = File::open(hint_path).expect("invalid hintfile path");
    let hints = Arc::new(Hints::from_file(&mut hintfile));
    elapsed_time(hintfile_start_time);
    let block_file_path = PathBuf::from(&blocks_dir);
    std::fs::create_dir(&block_file_path).expect("could not create block file directory");
    let stop_hash =
        consensus::deserialize::<BlockHash>(&hints.stop_hash()).expect("stop hash is not valid");
    tracing::info!("Assume valid hash: {stop_hash}");
    tracing::info!("Finding peers with DNS");
    let dns_start_time = Instant::now();
    let peers = bootstrap_dns(network);
    elapsed_time(dns_start_time);
    tracing::info!("Initializing bitcoin kernel");
    let kernel_start_time = Instant::now();
    let ctx = ContextBuilder::new()
        .chain_type(network.chain_type())
        .build()
        .unwrap();
    let options = ChainstateManagerOptions::new(&ctx, ".", "./blocks").unwrap();
    let chainman = ChainstateManager::new(options).unwrap();
    elapsed_time(kernel_start_time);
    let tip = chainman.best_header().height();
    tracing::info!("Kernel best header: {tip}");
    let chain = Arc::new(chainman);
    sync_block_headers(stop_hash, &peers, Arc::clone(&chain), network, timeout_conf);
    tracing::info!("Assume valid height: {}", chain.best_header().height());
    let (tx, rx) = channel();
    let main_routine_time = Instant::now();
    let mut accumulator_state = AccumulatorState::new(rx);
    let acc_task = std::thread::spawn(move || accumulator_state.verify());
    let peers = Arc::new(Mutex::new(peers));
    let mut tasks = Vec::new();
    let hashes = hashes_from_chain(Arc::clone(&chain), task_num);
    for (task_id, chunk) in hashes.into_iter().enumerate() {
        let chain = Arc::clone(&chain);
        let tx = tx.clone();
        let peers = Arc::clone(&peers);
        let hints = Arc::clone(&hints);
        let block_file_path = block_file_path.clone();
        let block_task = std::thread::spawn(move || {
            get_blocks_for_range(
                task_id as u32,
                timeout_conf,
                block_per_sec,
                ping_timeout,
                network,
                &block_file_path,
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
