use accumulator::Accumulator;
use bitcoin::{OutPoint, Txid};
use rusqlite::Connection;

const SELECT_STMT: &str = "SELECT txid, vout FROM utxos";

#[test]
fn test_static_utxo_set() {
    let mut acc = Accumulator::new();
    let conn = Connection::open("../contrib/signet_outpoints.sqlite").unwrap();
    let mut stmt = conn.prepare(SELECT_STMT).unwrap();
    let mut rows = stmt.query([]).unwrap();
    while let Some(row) = rows.next().unwrap() {
        let txid: String = row.get(0).unwrap();
        let vout: u32 = row.get(1).unwrap();
        let txid = txid.parse::<Txid>().unwrap();
        let outpoint = OutPoint { txid, vout };
        acc.spend(outpoint);
    }
    assert!(!acc.is_zero());
    let mut stmt = conn.prepare(SELECT_STMT).unwrap();
    let mut rows = stmt.query([]).unwrap();
    while let Some(row) = rows.next().unwrap() {
        let txid: String = row.get(0).unwrap();
        let vout: u32 = row.get(1).unwrap();
        let txid = txid.parse::<Txid>().unwrap();
        let outpoint = OutPoint { txid, vout };
        acc.add(outpoint);
    }
    assert!(acc.is_zero());
}
