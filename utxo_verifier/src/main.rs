use std::{collections::HashSet, net::IpAddr, sync::Arc, time::Duration};

use accumulator::Accumulator;
use bitcoin::{
    BlockHash, Network, block,
    p2p::{message::NetworkMessage, message_blockdata::GetHeadersMessage},
    secp256k1::rand::{seq::IteratorRandom, thread_rng},
};
use headers::{AcceptHeaderChanges, BlockTree};
use loader::update_acc_from_outpoint_set;
use p2p::{
    ConnectionBuilder, ConnectionContext,
    tokio_ext::{
        ReadError, TokioConnectionExt, TokioReadNetworkMessageExt, TokioWriteNetworkMessageExt,
    },
};
use peers::{
    PortExt, SeedsExt,
    dns::{DnsQuery, TokioDnsExt},
};
use tokio::{
    net::TcpStream,
    sync::{Mutex, oneshot},
    task::block_in_place,
    time::timeout,
};

mod headers;
mod loader;

const NETWORK: Network = Network::Bitcoin;
const SNAPSHOT_HEIGHT: u32 = 880_000;

async fn bootstrap(peers: Arc<Mutex<HashSet<IpAddr>>>) {
    let seeds = NETWORK.dns_seeds();
    for seed in seeds {
        let query = DnsQuery::new_cloudflare(seed).lookup_async().await;
        if let Ok(addrs) = query {
            tracing::info!("Adding IPs {} from {}", addrs.len(), seed);
            let mut table = peers.lock().await;
            table.extend(addrs);
        }
    }
    tracing::info!("DNS task exit");
}

async fn sync_header_chain(
    peers: Arc<Mutex<HashSet<IpAddr>>>,
    chain: Arc<Mutex<BlockTree>>,
    done: oneshot::Sender<()>,
) {
    let expected_port = NETWORK.port();
    loop {
        let lock = peers.lock().await;
        let peer = lock.iter().choose(&mut thread_rng()).copied();
        drop(lock);
        match peer {
            Some(ip_addr) => {
                let connection_result = ConnectionBuilder::new()
                    .no_cmpct_blocks()
                    .announce_by_inv()
                    .add_start_height(0)
                    .open_connection((ip_addr, expected_port))
                    .await;
                tracing::info!("Successful connection to peer {}", ip_addr);
                if let Ok((stream, ctx)) = connection_result {
                    request_headers_from_connection(stream, ctx, Arc::clone(&chain)).await;
                    let chain = chain.lock().await;
                    if chain.height().eq(&SNAPSHOT_HEIGHT) {
                        tracing::info!("Header sync task exit");
                        done.send(()).unwrap();
                        return;
                    }
                }
            }
            None => {
                tracing::info!("No peer available for header sync. Waiting a few seconds.");
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
    }
}

async fn request_headers_from_connection(
    mut stream: TcpStream,
    mut ctx: ConnectionContext,
    chain: Arc<Mutex<BlockTree>>,
) {
    let mut chain = chain.lock().await;
    loop {
        let stop_hash = BlockHash::from_byte_array([0u8; 32]);
        let request = GetHeadersMessage {
            version: 0x00,
            locator_hashes: vec![chain.tip_hash()],
            stop_hash,
        };
        tracing::info!("Header chain height: {}", chain.height());
        let message = NetworkMessage::GetHeaders(request);
        if let Err(e) = stream.write_message(message, &mut ctx).await {
            tracing::warn!("Header sync peer failed: {e}");
        }
        let timeout = timeout(
            Duration::from_secs(5),
            wait_for_header_response(&mut stream, &mut ctx),
        )
        .await;
        match timeout {
            Ok(responded) => match responded {
                Ok(headers) => {
                    for header in headers {
                        let apply_header = chain.accept_header(header);
                        match apply_header {
                            AcceptHeaderChanges::Accepted { connected_at } => {
                                // tracing::info!(
                                // "Accepted header: {}",
                                // connected_at.header.block_hash()
                                // );
                                if connected_at.height.eq(&SNAPSHOT_HEIGHT) {
                                    tracing::info!("Headers synced to snapshot height");
                                    return;
                                }
                            }
                            AcceptHeaderChanges::Reorganization {
                                accepted: _,
                                disconnected: _,
                            } => return,
                            AcceptHeaderChanges::ExtendedFork { connected_at: _ } => return,
                            AcceptHeaderChanges::Rejected(_) => return,
                            AcceptHeaderChanges::Duplicate => return,
                        }
                    }
                }
                Err(r) => {
                    tracing::info!("Header sync peer encountered an error: {r}");
                    return;
                }
            },
            Err(_) => {
                tracing::info!("Header sync peer timed out");
                return;
            }
        }
    }
}

async fn wait_for_header_response(
    stream: &mut TcpStream,
    ctx: &mut ConnectionContext,
) -> Result<Vec<block::Header>, ReadError> {
    loop {
        let data = stream.read_message(ctx).await?;
        if let Some(data) = data {
            match data {
                NetworkMessage::Ping(nonce) => {
                    tracing::info!("Ping {nonce}");
                    let _ = stream.write_message(NetworkMessage::Pong(nonce), ctx).await;
                }
                NetworkMessage::Headers(headers) => {
                    return Ok(headers);
                }
                other => tracing::info!("{}", other.cmd()),
            }
        }
    }
}

#[tokio::main]
async fn main() {
    let subscriber = tracing_subscriber::FmtSubscriber::new();
    tracing::subscriber::set_global_default(subscriber).unwrap();
    let mut args = std::env::args();
    let _ = args.next();
    let path = args
        .next()
        .expect("Provide a file path to the utxos.sqlite/outpoints.sqlite file");
    let mut acc = Accumulator::new();
    let peers = Arc::new(Mutex::new(HashSet::<IpAddr>::new()));
    tracing::info!("Starting DNS thread");
    let dns_mutex = Arc::clone(&peers);
    tokio::task::spawn(async move { bootstrap(dns_mutex).await });
    let chain = Arc::new(Mutex::new(BlockTree::from_genesis(NETWORK)));
    let header_chain_mutex = Arc::clone(&chain);
    let header_peer_mutex = Arc::clone(&peers);
    let (tx, rx) = oneshot::channel();
    tracing::info!("Background syncing header chain");
    tokio::task::spawn(async move {
        sync_header_chain(header_peer_mutex, header_chain_mutex, tx).await
    });
    tracing::info!("Loading OutPoint set and updating the accumulator");
    tokio::task::spawn_blocking(move || update_acc_from_outpoint_set(path, &mut acc));
    tracing::info!("Finished updating accumulator from snapshot");
    let _ = rx.await;
    tracing::info!("Done");
}
