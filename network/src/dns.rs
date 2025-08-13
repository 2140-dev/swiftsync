use std::{
    fmt::Display,
    io::Read,
    net::{IpAddr, Ipv4Addr, SocketAddr},
};

use crate::{encode_qname, rand_bytes};

const CLOUDFLARE: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)), 53);

const LOCAL_HOST: &str = "0.0.0.0:0";
const HEADER_BYTES: usize = 12;

const RECURSIVE_FLAGS: [u8; 2] = [
    0x01, 0x00, // Default flags with recursive resolver
];

const QTYPE: [u8; 4] = [
    0x00, 0x01, // QType: A Record
    0x00, 0x01, // IN
];

const COUNTS: [u8; 6] = [
    0x00, 0x00, // ANCOUNT
    0x00, 0x00, // NSCOUNT
    0x00, 0x00, // ARCOUNT
];

const A_RECORD: u16 = 0x01;
const A_CLASS: u16 = 0x01;
const EXPECTED_RDATA_LEN: u16 = 0x04;

pub struct DnsQuery {
    message_id: [u8; 2],
    message: Vec<u8>,
    question: Vec<u8>,
    resolver: SocketAddr,
}

impl DnsQuery {
    pub fn new(seed: &str, dns_resolver: SocketAddr) -> Self {
        // Build a header
        let message_id = rand_bytes();
        let mut message = message_id.to_vec();
        message.extend(RECURSIVE_FLAGS);
        message.push(0x00); // QDCOUNT
        message.push(0x01); // QDCOUNT
        message.extend(COUNTS);
        let mut question = encode_qname(seed, None);
        question.extend(QTYPE);
        message.extend_from_slice(&question);
        Self {
            message_id,
            message,
            question,
            resolver: dns_resolver,
        }
    }

    pub fn new_cloudflare(seed: &str) -> Self {
        let message_id = rand_bytes();
        let mut message = message_id.to_vec();
        message.extend(RECURSIVE_FLAGS);
        message.push(0x00); // QDCOUNT
        message.push(0x01); // QDCOUNT
        message.extend(COUNTS);
        let mut question = encode_qname(seed, None);
        question.extend(QTYPE);
        message.extend_from_slice(&question);
        Self {
            message_id,
            message,
            question,
            resolver: CLOUDFLARE,
        }
    }

    pub fn lookup(self) -> Result<Vec<IpAddr>, DnsResponseError> {
        let sock = std::net::UdpSocket::bind(LOCAL_HOST)?;
        sock.connect(self.resolver)?;
        sock.send(&self.message)?;
        let mut response_buf = [0u8; 512];
        let (amt, _src) = sock.recv_from(&mut response_buf)?;
        if amt < HEADER_BYTES {
            return Err(DnsResponseError::MalformedHeader);
        }
        let ips = self.parse_message(&response_buf[..amt])?;
        Ok(ips)
    }

    fn parse_message(&self, mut response: &[u8]) -> Result<Vec<IpAddr>, DnsResponseError> {
        let mut ips = Vec::with_capacity(10);
        let mut buf: [u8; 2] = [0, 0];
        response.read_exact(&mut buf)?; // Read 2 bytes
        if self.message_id != buf {
            return Err(DnsResponseError::MessageId);
        }
        // Read flags and ignore
        response.read_exact(&mut buf)?; // Read 4 bytes
        response.read_exact(&mut buf)?; // Read 6 bytes
        let _qdcount = u16::from_be_bytes(buf);
        response.read_exact(&mut buf)?; // Read 8 bytes
        let ancount = u16::from_be_bytes(buf);
        response.read_exact(&mut buf)?; // Read 10 bytes
        let _nscount = u16::from_be_bytes(buf);
        response.read_exact(&mut buf)?; // Read 12 bytes
        let _arcount = u16::from_be_bytes(buf);
        // The question should be repeated back to us
        let mut buf: Vec<u8> = vec![0; self.question.len()];
        response.read_exact(&mut buf)?;
        if self.question != buf {
            return Err(DnsResponseError::Question);
        }
        for _ in 0..ancount {
            let mut buf: [u8; 2] = [0, 0];
            // Read the compressed NAME field of the record and ignore
            response.read_exact(&mut buf)?;
            // Read the TYPE
            response.read_exact(&mut buf)?;
            let atype = u16::from_be_bytes(buf);
            // Read the CLASS
            response.read_exact(&mut buf)?;
            let aclass = u16::from_be_bytes(buf);
            let mut buf: [u8; 4] = [0, 0, 0, 0];
            // Read the TTL
            response.read_exact(&mut buf)?;
            let _ttl = u32::from_be_bytes(buf);
            let mut buf: [u8; 2] = [0, 0];
            // Read the RDLENGTH
            response.read_exact(&mut buf)?;
            let rdlength = u16::from_be_bytes(buf);
            // Read RDATA
            let mut rdata: Vec<u8> = vec![0; rdlength as usize];
            response.read_exact(&mut rdata)?;
            if atype == A_RECORD && aclass == A_CLASS && rdlength == EXPECTED_RDATA_LEN {
                ips.push(IpAddr::V4(Ipv4Addr::new(
                    rdata[0], rdata[1], rdata[2], rdata[3],
                )))
            }
        }
        Ok(ips)
    }
}

#[cfg(feature = "tokio")]
pub trait TokioDnsExt {
    #[allow(async_fn_in_trait)]
    async fn lookup_async(self) -> Result<Vec<IpAddr>, TokioDnsQueryError>;
}

#[cfg(feature = "tokio")]
impl TokioDnsExt for DnsQuery {
    async fn lookup_async(self) -> Result<Vec<IpAddr>, TokioDnsQueryError> {
        let sock = tokio::net::UdpSocket::bind(LOCAL_HOST).await?;
        sock.connect(self.resolver).await?;
        sock.send(&self.message).await?;
        let mut response_buf = [0u8; 512];
        let (amt, _src) = sock.recv_from(&mut response_buf).await?;
        if amt < HEADER_BYTES {
            return Err(TokioDnsQueryError::Protocol(
                DnsResponseError::MalformedHeader,
            ));
        }
        let ips = self.parse_message(&response_buf[..amt])?;
        Ok(ips)
    }
}

#[derive(Debug)]
pub enum DnsResponseError {
    MessageId,
    MalformedHeader,
    Question,
    Eof(std::io::Error),
}

impl Display for DnsResponseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Question => write!(f, "question section was not repeated back."),
            Self::MalformedHeader => write!(f, "the response header was undersized."),
            Self::MessageId => write!(f, "the response ID does not match the request."),
            Self::Eof(io) => write!(f, "std::io error: {io}"),
        }
    }
}

impl From<std::io::Error> for DnsResponseError {
    fn from(value: std::io::Error) -> Self {
        DnsResponseError::Eof(value)
    }
}

impl std::error::Error for DnsResponseError {}

#[cfg(feature = "tokio")]
#[derive(Debug)]
pub enum TokioDnsQueryError {
    Io(tokio::io::Error),
    Protocol(DnsResponseError),
}

#[cfg(feature = "tokio")]
impl Display for TokioDnsQueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(io) => write!(f, "{io}"),
            Self::Protocol(dns) => write!(f, "{dns}"),
        }
    }
}

#[cfg(feature = "tokio")]
impl std::error::Error for TokioDnsQueryError {}

#[cfg(feature = "tokio")]
impl From<tokio::io::Error> for TokioDnsQueryError {
    fn from(value: tokio::io::Error) -> Self {
        TokioDnsQueryError::Io(value)
    }
}

#[cfg(feature = "tokio")]
impl From<DnsResponseError> for TokioDnsQueryError {
    fn from(value: DnsResponseError) -> Self {
        TokioDnsQueryError::Protocol(value)
    }
}
