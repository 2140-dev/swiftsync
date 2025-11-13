use std::{fs::File, io::Write, path::PathBuf, str::FromStr};

use hintfile::write_compact_size;
use kernel::{ChainType, ChainstateManager, ChainstateManagerOptions, ContextBuilder, KernelError};

configure_me::include_config!();

fn chain_type_from_string(network: String) -> ChainType {
    match network.to_lowercase().as_ref() {
        "bitcoin" => ChainType::MAINNET,
        "signet" => ChainType::SIGNET,
        _ => panic!("supported chains are `bitcoin` or `signet`"),
    }
}

fn main() {
    let (config, _) = Config::including_optional_config_files::<&[&str]>(&[]).unwrap_or_exit();
    let chain_type = chain_type_from_string(config.network);
    let hintfile_path = PathBuf::from_str(&config.name).unwrap();
    let bitcoind = PathBuf::from_str(&config.bitcoin_dir).unwrap();
    let blocks_dir = bitcoind.join("blocks");
    let mut file = File::create(hintfile_path).unwrap();
    println!("Initializing");
    let ctx = ContextBuilder::new()
        .chain_type(chain_type)
        .build()
        .unwrap();
    let options = ChainstateManagerOptions::new(
        &ctx,
        bitcoind.to_str().unwrap(),
        blocks_dir.to_str().unwrap(),
    )
    .unwrap();
    let chainman = ChainstateManager::new(options).unwrap();
    println!("Chain state initialized");
    // Writing the chain tip allows the client to know where to stop
    let tip = chainman.block_index_tip();
    let stop_height = (tip.height() as u32).to_le_bytes();
    file.write_all(&stop_height)
        .expect("file cannot be written to");
    file.write_all(&tip.block_hash().hash)
        .expect("file cannot be written to");

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
                    curr = 0;
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
