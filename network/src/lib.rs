use std::{
    hash::{DefaultHasher, Hash, Hasher},
    time::SystemTime,
};

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

fn rand_bytes() -> [u8; 2] {
    let mut hasher = DefaultHasher::new();
    SystemTime::now().hash(&mut hasher);
    let mut hash = hasher.finish();
    hash ^= hash << 13;
    hash ^= hash >> 17;
    hash ^= hash << 5;
    hash.to_be_bytes()[..2].try_into().expect("trivial cast")
}

#[cfg(test)]
mod test {
    use crate::rand_bytes;

    #[test]
    fn test_rand_bytes() {
        rand_bytes();
    }
}
