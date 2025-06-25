use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use peers::dns::DnsQuery;

#[tokio::test]
async fn test_tokio_dns_ext() {
    use peers::dns::TokioDnsExt;
    let resolver = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(46, 166, 189, 67)), 53);
    let query = DnsQuery::new("seed.bitcoin.sipa.be", resolver);
    let addrs = query.lookup_async().await.unwrap();
    assert!(!addrs.is_empty());
}

#[test]
fn test_dns_blocking() {
    let resolver = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(46, 166, 189, 67)), 53);
    let query = DnsQuery::new("seed.bitcoin.sprovoost.nl", resolver);
    let addrs = query.lookup().unwrap();
    assert!(!addrs.is_empty());
}
