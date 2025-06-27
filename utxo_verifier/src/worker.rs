use std::{collections::HashSet, net::SocketAddr, sync::Arc};

use accumulator::Accumulator;
use bitcoin::{
    BlockHash, OutPoint,
    block::BlockUncheckedExt,
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
    tokio_ext::{TokioConnectionExt, TokioReadNetworkMessageExt, TokioWriteNetworkMessageExt},
};
use tokio::sync::{Mutex, mpsc};

use crate::NETWORK;

const REQUEST_SIZE: usize = 1000;

pub async fn fetch_blocks(
    acc: Arc<Mutex<Accumulator>>,
    hashes: Arc<Mutex<HashSet<BlockHash>>>,
    from: impl Into<SocketAddr>,
    done: mpsc::Sender<usize>,
    worker_id: usize,
) {
    let connection = ConnectionBuilder::new()
        .no_cmpct_blocks()
        .announce_by_inv()
        .change_network(NETWORK)
        .their_services_expected(ServiceFlags::NETWORK)
        .open_connection(from)
        .await;
    let (mut stream, mut ctx) = match connection {
        Ok((stream, ctx)) => (stream, ctx),
        Err(e) => {
            tracing::warn!("Failed to connect {e}");
            return;
        }
    };
    let hashes_lock = hashes.lock().await;
    let block_hashes = hashes_lock
        .iter()
        .choose_multiple(&mut thread_rng(), REQUEST_SIZE);
    let inv_payload = InventoryPayload(
        block_hashes
            .into_iter()
            .map(|hash| Inventory::Block(*hash))
            .collect(),
    );
    let mut checked_hashes: usize = 0;
    let get_blocks = NetworkMessage::GetData(inv_payload);
    if let Err(e) = stream.write_message(get_blocks, &mut ctx).await {
        tracing::warn!("Failed to write `getdata` {e}");
        return;
    }
    drop(hashes_lock);
    loop {
        let net_message = stream.read_message(&mut ctx).await;
        match net_message {
            Ok(message_opt) => {
                if let Some(message) = message_opt {
                    match message {
                        NetworkMessage::Ping(nonce) => {
                            tracing::info!("Ping {nonce}");
                            let _ = stream
                                .write_message(NetworkMessage::Pong(nonce), &mut ctx)
                                .await;
                        }
                        NetworkMessage::Block(block) => {
                            let checked = block.validate().unwrap();
                            let mut acc_lock = acc.lock().await;
                            for tx in checked.transactions() {
                                if !tx.is_coinbase() {
                                    for input in tx.inputs() {
                                        acc_lock.spend(input.previous_output);
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
                                    acc_lock.add(outpoint);
                                }
                            }
                            let hash = checked.block_hash();
                            let mut hashes_lock = hashes.lock().await;
                            checked_hashes += 1;
                            hashes_lock.remove(&hash);
                            if checked_hashes % 10 == 0 && checked_hashes != 0 {
                                tracing::info!(
                                    "Worker {} has added {} blocks",
                                    worker_id,
                                    checked_hashes
                                );
                            }
                            if checked_hashes % REQUEST_SIZE == 0 && checked_hashes != 0 {
                                let block_hashes = hashes_lock
                                    .iter()
                                    .choose_multiple(&mut thread_rng(), REQUEST_SIZE);
                                let inv_payload = InventoryPayload(
                                    block_hashes
                                        .into_iter()
                                        .map(|hash| Inventory::Block(*hash))
                                        .collect(),
                                );
                                let get_blocks = NetworkMessage::GetData(inv_payload);
                                if let Err(e) = stream.write_message(get_blocks, &mut ctx).await {
                                    tracing::warn!("Failed to write `getdata` {e}");
                                    return;
                                }
                            }
                            if hashes_lock.is_empty() {
                                let _ = done.send(worker_id).await;
                            }
                        }
                        other => tracing::info!("Worker {}: {}", worker_id, other.cmd()),
                    }
                }
            }
            Err(e) => {
                tracing::warn!("Connection closed {e}");
                return;
            }
        }
    }
}
