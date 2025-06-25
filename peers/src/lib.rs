use bitcoin::{Network, TestnetVersion};

pub mod dns;

fn encode_qname<S: AsRef<str>>(hostname: S, filter: Option<S>) -> Vec<u8> {
    let mut qname = Vec::new();
    let str = hostname.as_ref();
    if let Some(filter) = filter {
        let prefix = filter.as_ref();
        qname.push(prefix.len() as u8);
        qname.extend(prefix.as_bytes());
    }
    for label in str.split(".") {
        qname.push(label.len() as u8);
        qname.extend(label.as_bytes());
    }
    qname.push(0x00);
    qname
}

pub trait PortExt {
    fn port(&self) -> u16;
}

impl PortExt for Network {
    fn port(&self) -> u16 {
        match self {
            Self::Signet => 38333,
            Self::Bitcoin => 8333,
            Self::Regtest => 18444,
            Self::Testnet(TestnetVersion::V3) => 48333,
            Self::Testnet(TestnetVersion::V4) => 48333,
            _ => unreachable!()
        }
    }
}
