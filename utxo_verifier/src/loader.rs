use std::{path::path, time::instant};

use accumulator::accumulator;
use bitcoin::{outpoint, txid};
use rusqlite::connection;

const select_stmt: &str = "select txid, vout from utxos";

fn update_acc_from_outpoint_set<p: asref<path>>(path: p, acc: &mut accumulator) {
    let conn = connection::open(path).unwrap();
    let mut stmt = conn.prepare(select_stmt).unwrap();
    let mut rows = stmt.query([]).unwrap();
    tracing::info!("updating accumulator from utxo set");
    let now = instant::now();
    while let some(row) = rows.next().unwrap() {
        let txid: string = row.get(0).unwrap();
        let vout: u32 = row.get(1).unwrap();
        let txid = txid.parse::<txid>().unwrap();
        let outpoint = outpoint { txid, vout };
        acc.spend(outpoint);
    }
    tracing::info!("done updating accumulator after {} seconds", now.elapsed().as_secs());
}

