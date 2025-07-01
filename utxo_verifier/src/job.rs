use std::{
    collections::HashSet,
    net::IpAddr,
    sync::{Arc, mpsc},
    time::Duration,
};

use bitcoin::{
    BlockHash, OutPoint,
    p2p::{
        ServiceFlags,
        message::{InventoryPayload, NetworkMessage},
        message_blockdata::Inventory,
    },
    script::ScriptExt,
    secp256k1::rand::{seq::IteratorRandom, thread_rng},
    transaction::TransactionExt,
};
use p2p::{
    ConnectionBuilder,
    net::{ConnectionExt, ReadExt, WriteExt},
};
use peers::PortExt;

use crate::{AccumulatorUpdate, NETWORK};

pub fn fetch_blocks(
    sender: mpsc::Sender<Vec<AccumulatorUpdate>>,
    mut queue: Vec<Vec<BlockHash>>,
    peers: Arc<HashSet<IpAddr>>,
) {
    let mut batch = queue.pop().unwrap();
    loop {
        let any = peers.iter().choose(&mut thread_rng()).copied().unwrap();
        let connection = ConnectionBuilder::new()
            .no_cmpct_blocks()
            .announce_by_inv()
            .connection_timeout(Duration::from_secs(2))
            .change_network(NETWORK)
            .their_services_expected(ServiceFlags::NETWORK)
            .open_connection((any, NETWORK.port()));
        tracing::info!("Connecting to {any}");
        match connection {
            Ok((mut tcp_stream, mut ctx)) => {
                let _ = tcp_stream.set_read_timeout(Some(Duration::from_secs(3)));
                let inv = InventoryPayload(batch.iter().copied().map(Inventory::Block).collect());
                let getdata = NetworkMessage::GetData(inv);
                if tcp_stream.write_message(getdata, &mut ctx).is_err() {
                    tracing::warn!("Failed to write message");
                }
                loop {
                    let message = tcp_stream.read_message(&mut ctx);
                    match message {
                        Ok(message) => {
                            if let Some(message) = message {
                                match message {
                                    NetworkMessage::Block(b) => {
                                        let block = b.assume_checked(None);
                                        let mut updates =
                                            Vec::with_capacity(block.transactions().len());
                                        for tx in block.transactions() {
                                            if !tx.is_coinbase() {
                                                for input in tx.inputs() {
                                                    updates.push(AccumulatorUpdate::Spent(
                                                        input.previous_output,
                                                    ));
                                                }
                                            }
                                            let txid = tx.compute_txid();
                                            for (index, output) in tx.outputs().iter().enumerate() {
                                                if output.script_pubkey.is_op_return() {
                                                    continue;
                                                }
                                                let outpoint = OutPoint {
                                                    txid,
                                                    vout: index as u32,
                                                };
                                                updates.push(AccumulatorUpdate::Created(outpoint));
                                            }
                                        }
                                        sender.send(updates).unwrap();
                                        let this_hash = block.block_hash();
                                        batch.retain(|hash| hash.ne(&this_hash));
                                        if batch.is_empty() {
                                            match queue.pop() {
                                                Some(next) => {
                                                    batch = next;
                                                    tracing::info!("{any} has {} more batches", queue.len());
                                                    let inv = InventoryPayload(
                                                        batch
                                                            .iter()
                                                            .copied()
                                                            .map(Inventory::Block)
                                                            .collect(),
                                                    );
                                                    let getdata = NetworkMessage::GetData(inv);
                                                    if tcp_stream
                                                        .write_message(getdata, &mut ctx)
                                                        .is_err()
                                                    {
                                                        tracing::warn!("Failed to write message");
                                                        break;
                                                    }
                                                }
                                                None => {
                                                    tracing::info!("{any} finished a job");
                                                    return;
                                                }
                                            }
                                        }
                                    }
                                    NetworkMessage::Ping(nonce) => {
                                        let e = tcp_stream
                                            .write_message(NetworkMessage::Pong(nonce), &mut ctx);
                                        if e.is_err() {
                                            break;
                                        }
                                    }
                                    other => tracing::info!("{}", other.cmd()),
                                }
                            }
                        }
                        Err(_) => {
                            tracing::warn!("Connection to {any} closed");
                            break;
                        }
                    }
                }
            }
            Err(e) => {
                tracing::warn!("Connection to {any} failed: {e}");
            }
        }
    }
}

pub fn divide_jobs<T: Clone + Copy>(vec: Vec<T>, num_jobs: usize) -> Vec<Vec<T>> {
    assert!(num_jobs > 0);
    let n = vec.len();
    let mut jobs = vec![Vec::new(); num_jobs];
    #[allow(clippy::needless_range_loop)]
    for i in 0..num_jobs {
        let mut j = i;
        while j < n {
            jobs[i].push(vec[j]);
            j += num_jobs;
        }
    }
    jobs
}

#[cfg(test)]
mod tests {
    use super::divide_jobs;

    #[test]
    fn test_job_divison() {
        let data = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        let m = 3;
        let jobs = divide_jobs(data, m);
        let want =  vec![1, 4, 7, 10];
        let got = jobs.first().unwrap().clone();
        assert_eq!(want, got);
    }
}

