use bitcoin::{
    hashes::{sha256t, sha256t_tag},
    OutPoint,
};

sha256t_tag! {
    pub struct SwiftSyncTag = hash_str("SwiftSync");
}

/// A simple accumulator that can add and remove elements from a set. If all elements have been
/// added and removed the equivalent amount of times, the accumulator is zero. In the context of
/// bitcoin, this is used to add and remove hashes of [`OutPoint`] data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, std::hash::Hash)]
pub struct Accumulator {
    high: u128,
    low: u128,
}

/// Hash a bitcoin [`OutPoint`] to add or spend it from an accumulator.
pub fn hash_outpoint(outpoint: OutPoint) -> [u8; 32] {
    let mut input = [0u8; 36];
    let txid = outpoint.txid.to_byte_array();
    let index = outpoint.vout.to_le_bytes();
    input[..32].copy_from_slice(&txid);
    input[32..].copy_from_slice(&index);
    sha256t::hash::<SwiftSyncTag>(&input).to_byte_array()
}

fn split_in_half(a: [u8; 32]) -> ([u8; 16], [u8; 16]) {
    let mut high = [0_u8; 16];
    let mut low = [0_u8; 16];
    high.copy_from_slice(&a[..16]);
    low.copy_from_slice(&a[16..]);
    (high, low)
}

impl Accumulator {
    /// The zero accumulator
    pub const ZERO: Accumulator = Accumulator { high: 0, low: 0 };

    /// Build a new accumulator.
    pub const fn new() -> Self {
        Self::ZERO
    }

    /// Add an [`OutPoint`] to the set. Normally used when coins are created in a block.
    // ref: https://github.com/rust-bitcoin/rust-bitcoin/blob/7bbb9085c63dc69e9da16ec9c11c698d6236c95c/bitcoin/src/pow.rs#L658
    pub fn add(&mut self, outpoint: OutPoint) {
        let hash = hash_outpoint(outpoint);
        let (big, little) = Self::create_rhs(hash);
        let (high, low) = Self::add_internal(self.high, self.low, big, little);
        *self = Self { high, low };
    }

    /// Add a pre-hashed outpoint to the accumulator.
    pub fn add_hashed_outpoint(&mut self, hash: [u8; 32]) {
        let (big, little) = Self::create_rhs(hash);
        let (high, low) = Self::add_internal(self.high, self.low, big, little);
        *self = Self { high, low };
    }

    /// Spend the inputs in a block by subtracing them from the accumulator.
    pub fn spend(&mut self, outpoint: OutPoint) {
        let hash = hash_outpoint(outpoint);
        let (high, low) = Self::create_rhs(hash);
        let high_inv = !high;
        let low_inv = !low;
        let (high, low) = Self::add_internal(self.high, self.low, high_inv, low_inv);
        let (high, low) = Self::add_internal(high, low, 0, 1);
        *self = Self { high, low }
    }

    /// Spend a pre-hashed outpoint from the accumulator.
    pub fn spend_hashed_outpoint(&mut self, hash: [u8; 32]) {
        let (high, low) = Self::create_rhs(hash);
        let high_inv = !high;
        let low_inv = !low;
        let (high, low) = Self::add_internal(self.high, self.low, high_inv, low_inv);
        let (high, low) = Self::add_internal(high, low, 0, 1);
        *self = Self { high, low }
    }

    // Add LHS to RHS, wrapping around if necessary
    fn add_internal(lhs_high: u128, lhs_low: u128, rhs_high: u128, rhs_low: u128) -> (u128, u128) {
        let high = lhs_high.wrapping_add(rhs_high);
        let mut ret_high = high;
        let (low, low_bits_overflow) = lhs_low.overflowing_add(rhs_low);
        if low_bits_overflow {
            // Carry
            ret_high = ret_high.wrapping_add(1);
        }
        (ret_high, low)
    }

    fn create_rhs(hash: [u8; 32]) -> (u128, u128) {
        let (high, low) = split_in_half(hash);
        let big = u128::from_be_bytes(high);
        let little = u128::from_be_bytes(low);
        (big, little)
    }

    /// Is the interal state of the accumulator zero. This may be checked when you believe all
    /// [`OutPoint`] have been added and removed equivalent times.
    pub fn is_zero(&self) -> bool {
        self.low.eq(&0) && self.high.eq(&0)
    }
}

impl Default for Accumulator {
    fn default() -> Self {
        Self::ZERO
    }
}

#[cfg(test)]
mod tests {
    use bitcoin::{
        secp256k1::rand::{thread_rng, RngCore},
        Txid,
    };

    use super::*;

    const TXID_ONE: Txid = Txid::COINBASE_PREVOUT;

    fn make_rand_txid() -> Txid {
        let mut rng = thread_rng();
        let mut txid_two_bytes = [0u8; 32];
        rng.fill_bytes(&mut txid_two_bytes);
        Txid::from_byte_array(txid_two_bytes)
    }

    fn make_five_outpoint() -> [OutPoint; 5] {
        let txid_two = make_rand_txid();
        let txid_three = make_rand_txid();
        let txid_four = make_rand_txid();
        let txid_five = make_rand_txid();
        let outpoint_one = OutPoint {
            txid: TXID_ONE,
            vout: 0,
        };
        let outpoint_two = OutPoint {
            txid: txid_two,
            vout: 213,
        };
        let outpoint_three = OutPoint {
            txid: txid_three,
            vout: 432,
        };
        let outpoint_four = OutPoint {
            txid: txid_four,
            vout: 3212,
        };
        let outpoint_five = OutPoint {
            txid: txid_five,
            vout: 2,
        };
        [
            outpoint_one,
            outpoint_two,
            outpoint_three,
            outpoint_four,
            outpoint_five,
        ]
    }

    #[test]
    fn test_accumulator_is_zero() {
        let mut acc = Accumulator::default();
        let [outpoint_one, outpoint_two, outpoint_three, outpoint_four, outpoint_five] =
            make_five_outpoint();
        // Add the members
        acc.add(outpoint_one);
        acc.add(outpoint_two);
        acc.add(outpoint_five);
        acc.add(outpoint_four);
        acc.add(outpoint_three);
        assert!(!acc.is_zero());
        // Take away the members
        acc.spend(outpoint_two);
        acc.spend(outpoint_five);
        acc.spend(outpoint_three);
        acc.spend(outpoint_four);
        acc.spend(outpoint_one);
        assert!(acc.is_zero());
    }

    #[test]
    fn test_same_state() {
        let [outpoint_one, outpoint_two, outpoint_three, outpoint_four, outpoint_five] =
            make_five_outpoint();
        let mut acc_ref = Accumulator::default();
        acc_ref.add(outpoint_two);
        acc_ref.add(outpoint_four);
        let mut acc_cmp = Accumulator::default();
        acc_cmp.add(outpoint_one);
        acc_cmp.add(outpoint_two);
        acc_cmp.add(outpoint_three);
        acc_cmp.add(outpoint_four);
        acc_cmp.add(outpoint_five);
        // Spend one, three, five
        acc_cmp.spend(outpoint_three);
        acc_cmp.spend(outpoint_one);
        acc_cmp.spend(outpoint_five);
        assert!(acc_ref.eq(&acc_cmp));
    }

    #[test]
    fn test_prehashing() {
        let [outpoint_one, outpoint_two, outpoint_three, outpoint_four, outpoint_five] =
            make_five_outpoint();
        let hash_one = hash_outpoint(outpoint_one);
        let hash_two = hash_outpoint(outpoint_two);
        let hash_three = hash_outpoint(outpoint_three);
        let hash_four = hash_outpoint(outpoint_four);
        let hash_five = hash_outpoint(outpoint_five);
        let mut acc = Accumulator::default();
        acc.add_hashed_outpoint(hash_five);
        acc.add_hashed_outpoint(hash_four);
        acc.add_hashed_outpoint(hash_one);
        acc.add_hashed_outpoint(hash_two);
        acc.add_hashed_outpoint(hash_three);
        acc.spend_hashed_outpoint(hash_five);
        acc.spend_hashed_outpoint(hash_four);
        acc.spend_hashed_outpoint(hash_one);
        acc.spend_hashed_outpoint(hash_two);
        acc.spend_hashed_outpoint(hash_three);
    }
}
