use std::net::SocketAddrV4;

use bitcoin::{BlockHash, Network};
use corepc_node::{exe_path, P2P};
use p2p::{message::NetworkMessage, ServiceFlags};
use swiftsync_p2p::ConnectionBuilder;

#[derive(Debug, Clone)]
struct TestNodeBuilder<'a> {
    conf: corepc_node::Conf<'a>,
}

impl<'a> TestNodeBuilder<'a> {
    fn new() -> Self {
        let mut conf = corepc_node::Conf::default();
        conf.p2p = P2P::Yes;
        conf.args.push("--listen=1");
        conf.args.push("--rest=1");
        conf.args.push("--server=1");
        Self { conf }
    }

    fn push_arg(mut self, arg: &'a str) -> Self {
        self.conf.args.push(arg);
        self
    }

    fn start(self) -> (corepc_node::Node, SocketAddrV4) {
        let path = exe_path().unwrap();
        let bitcoind = corepc_node::Node::with_conf(path, &self.conf).unwrap();
        let socket_addr = bitcoind.params.p2p_socket.unwrap();
        (bitcoind, socket_addr)
    }
}

// Standard library tests

#[test]
fn does_handshake() {
    use swiftsync_p2p::net::ConnectionExt;
    let (mut bitcoind, socket_addr) = TestNodeBuilder::new().start();
    let _ = ConnectionBuilder::new()
        .change_network(Network::Regtest)
        .open_connection(socket_addr)
        .unwrap();
    bitcoind.stop().unwrap();
}

#[test]
fn filters_unsupported_messages() {
    use swiftsync_p2p::net::{ConnectionExt, WriteExt};
    let (mut bitcoind, socket_addr) = TestNodeBuilder::new().start();
    let (mut tcp_stream, mut ctx) = ConnectionBuilder::new()
        .change_network(Network::Regtest)
        .open_connection(socket_addr)
        .unwrap();
    let err = tcp_stream.write_message(NetworkMessage::Verack, &mut ctx);
    assert!(err.is_err());
    let err = tcp_stream.write_message(NetworkMessage::MemPool, &mut ctx);
    assert!(err.is_err());
    let err = tcp_stream.write_message(
        NetworkMessage::GetCFilters(p2p::message_filter::GetCFilters {
            filter_type: 0x00,
            start_height: 0.into(),
            stop_hash: BlockHash::from_byte_array([0; 32]),
        }),
        &mut ctx,
    );
    assert!(err.is_err());
    bitcoind.stop().unwrap();
}

#[test]
fn enforces_desired_services() {
    use swiftsync_p2p::net::ConnectionExt;
    let (mut bitcoind, socket_addr) = TestNodeBuilder::new().start();
    let err = ConnectionBuilder::new()
        .change_network(Network::Regtest)
        .their_services_expected(ServiceFlags::COMPACT_FILTERS)
        .open_connection(socket_addr);
    assert!(err.is_err());
    bitcoind.stop().unwrap();
    let (mut bitcoind, socket_addr) = TestNodeBuilder::new()
        .push_arg("--blockfilterindex")
        .push_arg("--peerblockfilters")
        .start();
    let ok = ConnectionBuilder::new()
        .change_network(Network::Regtest)
        .their_services_expected(ServiceFlags::COMPACT_FILTERS)
        .open_connection(socket_addr);
    assert!(ok.is_ok());
    bitcoind.stop().unwrap();
}

// Tokio tests

#[tokio::test]
async fn does_handshake_async() {
    use swiftsync_p2p::tokio_ext::TokioConnectionExt;
    let (mut bitcoind, socket_addr) = TestNodeBuilder::new().start();
    let _ = ConnectionBuilder::new()
        .change_network(Network::Regtest)
        .open_connection(socket_addr)
        .await
        .unwrap();
    bitcoind.stop().unwrap();
}
