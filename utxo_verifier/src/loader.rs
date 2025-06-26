use std::{path::Path, time::Instant};

use accumulator::Accumulator;
use bitcoin::{OutPoint, Txid};
use rusqlite::Connection;

const SELECT_STMT: &str = "SELECT txid, vout FROM utxos";

pub fn update_acc_from_outpoint_set<P: AsRef<Path>>(path: P, acc: &mut Accumulator) {
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
        acc.spend(outpoint);
        outpoints_spent += 1;
        if outpoints_spent % 1_000_000 == 0 {
            tracing::info!("{outpoints_spent} OutPoints added to the accumulator");
        }
    }
    tracing::info!("Done spending UTXOs in {} seconds", now.elapsed().as_secs());
}
