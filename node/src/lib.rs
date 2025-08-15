use std::{net::SocketAddr, sync::mpsc::Receiver, time::Instant};

use accumulator::{Accumulator, AccumulatorUpdate};
use bitcoin::Network;
use network::dns::DnsQuery;
use p2p::SeedsExt;

pub fn elapsed_time(then: Instant) {
    let duration_sec = then.elapsed().as_secs_f64();
    tracing::info!("Elapsed time {duration_sec} seconds");
}

pub trait PortExt {
    fn port(&self) -> u16;
}

impl PortExt for Network {
    fn port(&self) -> u16 {
        match self {
            Network::Signet => 38333,
            Network::Bitcoin => 8333,
            _ => unimplemented!(),
        }
    }
}

#[derive(Debug)]
pub struct AccumulatorState {
    acc: Accumulator,
    update_rx: Receiver<AccumulatorUpdate>,
}

impl AccumulatorState {
    fn new(rx: Receiver<AccumulatorUpdate>) -> Self {
        Self {
            acc: Accumulator::new(),
            update_rx: rx,
        }
    }

    fn verify(&mut self) -> bool {
        while let Ok(update) = self.update_rx.recv() {
            self.acc.update(update);
        }
        self.acc.is_zero()
    }
}

pub fn bootstrap_dns(network: Network) -> Vec<SocketAddr> {
    let mut all_hosts = Vec::new();
    for seed in network.seeds() {
        let hosts = DnsQuery::new_cloudflare(seed).lookup().unwrap_or_default();
        all_hosts.extend_from_slice(&hosts);
    }
    all_hosts
        .into_iter()
        .map(|host| SocketAddr::new(host, network.port()))
        .collect()
}
