use std::{fs::File, sync::Arc};

use bitcoin::consensus;
use kernel::{ChainType, ChainstateManager, ChainstateManagerOptions, ContextBuilder, KernelError};

fn main() {
    let mut file = File::create("./signet.hints").unwrap();

    let mut args = std::env::args();
    let _ = args.next();
    let data_dir = args.next().expect("Usage: <path_to_bitcoin_dir>");
    let mut blocks_dir = data_dir.clone();
    blocks_dir.push_str("/blocks");
    println!("Initializing");
    let ctx = ContextBuilder::new()
        .chain_type(ChainType::SIGNET)
        .build()
        .unwrap();
    let options = ChainstateManagerOptions::new(&ctx, &data_dir, &blocks_dir).unwrap();
    let context = Arc::new(ctx);
    let chainman = ChainstateManager::new(options, context).unwrap();
    println!("Chain state initialized");
    let genesis = chainman.get_block_index_genesis();
    let mut current = chainman.get_next_block_index(genesis).unwrap();
    loop {
        let block = chainman.read_block_data(&current).unwrap();
        let bytes: Vec<u8> = block.into();
        let block = consensus::deserialize::<bitcoin::Block>(&bytes).unwrap();
        let (_, transactions) = block.into_parts();
        println!("On block {}", current.height());
        let mut delta: u64 = 0;
        let mut block_offsets: Vec<u64> = Vec::new();
        for tx in transactions {}
        match chainman.get_next_block_index(current) {
            Ok(next) => current = next,
            Err(KernelError::OutOfBounds) => break,
            Err(e) => panic!("{e}"),
        }
    }
}
