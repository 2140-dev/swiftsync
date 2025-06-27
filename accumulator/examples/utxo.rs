use std::{path::Path, time::Instant};

use accumulator::Accumulator;
use bitcoin::{OutPoint, Txid};
use rusqlite::Connection;

const SELECT_STMT: &str = "SELECT txid, vout FROM utxos";

fn update_acc_from_outpoint_set<P: AsRef<Path>>(path: P, acc: &mut Accumulator) {
    let conn = Connection::open(path).unwrap();
    let mut stmt = conn.prepare(SELECT_STMT).unwrap();
    let mut rows = stmt.query([]).unwrap();
    let now = Instant::now();
    println!("Spending UTXOS from accumulator...");
    while let Some(row) = rows.next().unwrap() {
        let txid: String = row.get(0).unwrap();
        let vout: u32 = row.get(1).unwrap();
        let txid = txid.parse::<Txid>().unwrap();
        let outpoint = OutPoint { txid, vout };
        acc.spend(outpoint);
    }
    println!("Done spending UTXOs in {} seconds", now.elapsed().as_secs());
    println!("Accumulator is zero: {}", acc.is_zero());
    let mut stmt = conn.prepare(SELECT_STMT).unwrap();
    let mut rows = stmt.query([]).unwrap();
    let now = Instant::now();
    println!("Adding UTXOS to accumulator...");
    while let Some(row) = rows.next().unwrap() {
        let txid: String = row.get(0).unwrap();
        let vout: u32 = row.get(1).unwrap();
        let txid = txid.parse::<Txid>().unwrap();
        let outpoint = OutPoint { txid, vout };
        acc.add(outpoint);
    }
    println!("Done adding UTXOs in {} seconds", now.elapsed().as_secs());
    println!("Accumulator is zero: {}", acc.is_zero());
}

fn main() {
    let mut acc = Accumulator::new();
    update_acc_from_outpoint_set("../signet_outpoints.sqlite", &mut acc);
}
