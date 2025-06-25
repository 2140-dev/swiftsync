use bitcoin::{BlockHeader, block::HeaderExt, p2p::message_blockdata::Inventory};

pub(crate) trait ValidationExt {
    fn is_valid(&self) -> bool;
}

impl ValidationExt for Vec<BlockHeader> {
    fn is_valid(&self) -> bool {
        self.iter()
            .zip(self.iter().skip(1))
            .all(|(first, second)| first.block_hash().eq(&second.prev_blockhash))
            && !self.iter().any(|header| {
                let target = header.target();
                let valid_pow = header.validate_pow(target);
                valid_pow.is_err()
            })
    }
}

impl ValidationExt for Vec<Inventory> {
    fn is_valid(&self) -> bool {
        self.len() < 50_000
    }
}
