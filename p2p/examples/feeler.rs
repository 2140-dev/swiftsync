use std::{net::SocketAddr, time::Duration};

use bitcoin::{
    Network,
    secp256k1::rand::{seq::SliceRandom, thread_rng},
};
use peers::{PortExt, SeedsExt, dns::DnsQuery};
use swiftsync_p2p::{ConnectionBuilder, net::ConnectionExt};

const NETWORK: Network = Network::Signet;

fn main() {
    let subscriber = tracing_subscriber::FmtSubscriber::new();
    tracing::subscriber::set_global_default(subscriber).unwrap();
    tracing::info!("Finding a peer with DNS");
    let recommended_seed = NETWORK.dns_seeds().first().copied().unwrap();
    let dns = DnsQuery::new_cloudflare(recommended_seed).lookup().unwrap();
    let any = dns.choose(&mut thread_rng()).copied().unwrap();
    let socket_addr = SocketAddr::new(any, NETWORK.port());
    tracing::info!("Attempting a connection");
    let connection = ConnectionBuilder::new()
        .set_user_agent("/bitcoin-feeler:0.1.0".to_string())
        .connection_timeout(Duration::from_millis(3500))
        .change_network(NETWORK)
        .open_connection(socket_addr);
    match connection {
        Ok(_) => tracing::info!("Connection successful!"),
        Err(e) => tracing::warn!("Connection failed {e:?}"),
    }
}
