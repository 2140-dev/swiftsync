use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use peers::dns::{DnsQuery, TokioDnsExt};

#[tokio::test]
async fn test_tokio_dns_ext() {
    let resolver = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(46, 166, 189, 67)), 53);
    let query = DnsQuery::new("seed.bitcoin.sipa.be", resolver);
    let addrs = query.lookup().await.unwrap();
    assert!(!addrs.is_empty());
}
