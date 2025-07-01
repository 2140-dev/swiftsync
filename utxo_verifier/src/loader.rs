use std::{path::Path, time::Instant};

use accumulator::Accumulator;
use bitcoin::{OutPoint, Txid};
use std::collections::BTreeMap;

use bitcoin::{consensus, BlockHash};
use rusqlite::Connection;

const SELECT_STMT: &str = "SELECT txid, vout FROM utxos";

pub fn update_acc_from_outpoint_set<P: AsRef<Path>>(path: P) -> Accumulator {
    let mut acc = Accumulator::new();
    let conn = Connection::open(path).unwrap();
    let mut stmt = conn.prepare(SELECT_STMT).unwrap();
    let mut rows = stmt.query([]).unwrap();
    tracing::info!("Spending UTXOs from the accumulator");
    let mut outpoints_spent = 0;
    let now = Instant::now();
    while let Some(row) = rows.next().unwrap() {
        let txid: String = row.get(0).unwrap();
        let vout: u32 = row.get(1).unwrap();
        let txid = txid.parse::<Txid>().unwrap();
        let outpoint = OutPoint { txid, vout };
        acc.add(outpoint);
        outpoints_spent += 1;
        if outpoints_spent % 1_000_000 == 0 {
            tracing::info!("{outpoints_spent} OutPoints added to the accumulator");
        }
    }
    tracing::info!("Done spending UTXOs in {} seconds", now.elapsed().as_secs());
    acc
}

pub fn get_block_hashes_from_store<P: AsRef<Path>>(
    path: P,
    assume_valid: BlockHash,
) -> BTreeMap<u32, BlockHash> {
    let mut hashes = BTreeMap::new();
    let conn = Connection::open(path).unwrap();
    let mut stmt = conn
        .prepare("SELECT height, block_hash FROM headers")
        .unwrap();
    tracing::info!("Loading hashes from storage");
    let now = Instant::now();
    let mut rows = stmt.query([]).unwrap();
    while let Some(row) = rows.next().unwrap() {
        let height: u32 = row.get(0).unwrap();
        let hash: [u8; 32] = row.get(1).unwrap();
        if height == 0 {
            continue;
        }
        let block_hash: BlockHash = consensus::deserialize(&hash).unwrap();
        hashes.insert(height, block_hash);
        if block_hash.eq(&assume_valid) {
            let secs = now.elapsed().as_secs();
            tracing::info!("Done loading hashes in {} seconds", secs);
            return hashes;
        }
    }
    panic!("expected assume valid hash");
}
