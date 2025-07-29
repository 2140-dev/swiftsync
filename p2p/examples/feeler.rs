use std::{net::SocketAddr, time::Duration};

use bitcoin::{
    secp256k1::rand::{seq::SliceRandom, thread_rng},
    Network,
};
use p2p::message_network::UserAgent;
use peers::{dns::DnsQuery, PortExt, SeedsExt};
use swiftsync_p2p::{net::ConnectionExt, ConnectionBuilder};

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
        .set_user_agent(UserAgent::from_nonstandard("bitcoin-feeler"))
        .connection_timeout(Duration::from_millis(3500))
        .change_network(NETWORK)
        .open_feeler(socket_addr);
    match connection {
        Ok(f) => {
            tracing::info!(
                "Connection successful: Advertised protocol version {:?}, Adveristed services {}",
                f.protocol_version,
                f.services
            );
        }
        Err(e) => tracing::warn!("Connection failed {e:?}"),
    }
}
