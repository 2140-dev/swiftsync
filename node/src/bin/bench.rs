// This binary is used to assess the impact of accumulator designs

use std::{path::PathBuf, str::FromStr, time::Instant};

use bitcoin::{consensus, OutPoint};
use kernel::{ChainType, ChainstateManager, ChainstateManagerOptions, ContextBuilder};
use node::elapsed_time;

fn main() {
    let subscriber = tracing_subscriber::FmtSubscriber::new();
    tracing::subscriber::set_global_default(subscriber).unwrap();
    let home = std::env::var("HOME").unwrap();
    let path_buf = PathBuf::from_str(&home).unwrap();
    let bitcoin_dir = path_buf.join(".bitcoin");
    let blocks_dir = bitcoin_dir.join("blocks");
    let ctx = ContextBuilder::new()
        .chain_type(ChainType::MAINNET)
        .build()
        .unwrap();
    let opts = ChainstateManagerOptions::new(
        &ctx,
        bitcoin_dir.to_str().unwrap(),
        blocks_dir.to_str().unwrap(),
    )
    .unwrap();
    let chainman = ChainstateManager::new(opts).unwrap();
    chainman.import_blocks().unwrap();
    let mut acc = accumulator::Accumulator::new();
    let mut tip = chainman.block_index_tip();
    tracing::info!("Starting accumulator bench");
    let start = Instant::now();
    while let Ok(next) = tip.prev() {
        tracing::info!("process block {}", next.height());
        let block = chainman.read_block_data(&next).unwrap();
        let (_, transactions) = consensus::deserialize::<bitcoin::Block>(&block.to_bytes())
            .unwrap()
            .into_parts();
        for tx in transactions {
            let txid = tx.compute_txid();
            for input in tx.inputs {
                let outpoint = input.previous_output;
                acc.spend(outpoint);
            }
            for vout in 0..tx.outputs.len() {
                let outpoint = OutPoint {
                    txid,
                    vout: vout as u32,
                };
                acc.add(outpoint);
            }
        }
        tip = next;
    }
    elapsed_time(start);
}
