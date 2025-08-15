use std::{fs::File, io::Write, sync::Arc};

use hintfile::write_compact_size;
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
    let _context = Arc::new(ctx);
    let chainman = ChainstateManager::new(options).unwrap();
    println!("Chain state initialized");
    // Writing the chain tip allows the client to know where to stop
    let tip = chainman.block_index_tip().block_hash().hash;
    file.write_all(&tip).expect("file cannot be written to");

    let genesis = chainman.block_index_genesis();
    let mut current = chainman.next_block_index(genesis).unwrap();
    loop {
        let block = chainman.read_block_data(&current).unwrap();
        println!("Block {} ...", current.height());
        let mut block_unspents = Vec::new();
        let mut curr = 0;
        for i in 0..block.transaction_count() {
            let transaction = block.transaction(i).unwrap();
            for vout in 0..transaction.output_count() {
                if chainman.have_coin(&transaction, vout) {
                    println!("Found coin at offset {curr}");
                    block_unspents.push(curr);
                }
                curr += 1;
            }
        }

        // Overflows 32 bit machines
        let len_encode = block_unspents.len() as u64;
        write_compact_size(len_encode, &mut file).expect("unexpected EOF");
        for offset in block_unspents {
            write_compact_size(offset, &mut file).expect("unexpected EOF");
        }
        match chainman.next_block_index(current) {
            Ok(next) => current = next,
            Err(KernelError::OutOfBounds) => break,
            Err(e) => panic!("{e}"),
        }
    }
}
