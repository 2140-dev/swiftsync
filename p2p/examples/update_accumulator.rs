use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    time::Instant,
};

use accumulator::Accumulator;
use bitcoin::{
    BlockHash, Network, OutPoint,
    block::BlockUncheckedExt,
    p2p::{
        ServiceFlags,
        message::NetworkMessage,
        message_blockdata::{GetBlocksMessage, Inventory},
    },
    secp256k1::rand::{seq::SliceRandom, thread_rng},
};
use peers::{
    PortExt,
    dns::{DnsQuery, TokioDnsExt},
};
use swiftsync_p2p::{
    ConnectionBuilder,
    tokio_ext::{TokioConnectionExt, TokioReadNetworkMessageExt, TokioWriteNetworkMessageExt},
};

const DNS_SEED: &str = "seed.bitcoin.sprovoost.nl";
const NETWORK: Network = Network::Bitcoin;
const CLOUDFLARE: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)), 53);
const START_HEIGHT: i32 = 900_000;

#[tokio::main]
async fn main() {
    let subscriber = tracing_subscriber::FmtSubscriber::new();
    tracing::subscriber::set_global_default(subscriber).unwrap();

    let mut acc = Accumulator::new();
    let locator_hash = "000000000000000000010538edbfd2d5b809a33dd83f284aeea41c6d0d96968a"
        .parse::<BlockHash>()
        .unwrap();
    let zero = BlockHash::from_byte_array([0u8; 32]);
    tracing::info!("Configuring connection requirements");
    let connection_builder = ConnectionBuilder::new()
        .change_network(NETWORK)
        .add_start_height(START_HEIGHT)
        .set_user_agent("/example-accumulator:0.1.0".to_string())
        .no_cmpct_blocks()
        .announce_by_inv()
        .their_services_expected(ServiceFlags::NETWORK);
    tracing::info!("Querying DNS for peers");
    let dns = DnsQuery::new(DNS_SEED, CLOUDFLARE)
        .lookup_async()
        .await
        .unwrap();
    tracing::info!("Connecting to the first result");
    let first = dns.choose(&mut thread_rng()).unwrap();
    let peer = SocketAddr::new(*first, NETWORK.port());
    let (mut stream, mut ctx) = connection_builder.open_connection(peer).await.unwrap();
    tracing::info!("Completed version handshake");
    let get_blocks_request = GetBlocksMessage::new(vec![locator_hash], zero);
    let message = NetworkMessage::GetBlocks(get_blocks_request);
    tracing::info!("Requesting blocks");
    stream.write_message(message, &mut ctx).await.unwrap();
    tracing::info!("Waiting for response");
    loop {
        let response = stream.read_message(&mut ctx).await.unwrap();
        if let Some(message) = response {
            match message {
                NetworkMessage::Ping(nonce) => stream
                    .write_message(NetworkMessage::Pong(nonce), &mut ctx)
                    .await
                    .unwrap(),
                NetworkMessage::Inv(data) => {
                    if data
                        .0
                        .iter()
                        .any(|inv| matches!(inv, Inventory::Block(_) | Inventory::WitnessBlock(_)))
                    {
                        let getdata = NetworkMessage::GetData(data);
                        stream.write_message(getdata, &mut ctx).await.unwrap();
                    }
                }
                NetworkMessage::Block(block) => {
                    let checked = block.validate().unwrap();
                    let hash = checked.block_hash();
                    tracing::info!("Validated block: {hash}");
                    let now = Instant::now();
                    tracing::info!("Updating the accumulator");
                    for tx in checked.transactions() {
                        for input in &tx.input {
                            let outpoint = input.previous_output;
                            acc.spend(outpoint);
                        }
                        let txid = tx.compute_txid();
                        for ind in 0..tx.output.len() {
                            let outpoint = OutPoint {
                                txid,
                                vout: ind as u32,
                            };
                            acc.add(outpoint);
                        }
                    }
                    tracing::info!(
                        "Updated accumulator in {} milliseconds",
                        now.elapsed().as_millis()
                    );
                    return;
                }
                other => tracing::info!("{}", other.cmd()),
            }
        }
    }
}
