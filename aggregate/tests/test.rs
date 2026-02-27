use aggregate::Aggregate;
use bitcoin::{OutPoint, Txid};

const TEST_ITERS: usize = 10_000;
const TEST_SEED: u64 = 420;

struct Rng {
    state: u64,
}

impl Rng {
    fn new(seed: u64) -> Self {
        Rng {
            state: if seed == 0 { 1 } else { seed },
        }
    }

    fn next_u64(&mut self) -> u64 {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 7;
        self.state ^= self.state << 17;
        self.state
    }

    fn next_32_bytes(&mut self) -> [u8; 32] {
        let mut out = [0u8; 32];
        for chunk in out.chunks_exact_mut(8) {
            chunk.copy_from_slice(&self.next_u64().to_le_bytes());
        }
        out
    }
}

#[test]
fn test_static_utxo_set() {
    let mut acc = Aggregate::new();
    let mut rng = Rng::new(TEST_SEED);
    let mut outpoints = Vec::with_capacity(TEST_ITERS);
    for _ in 0..TEST_ITERS {
        let txid = Txid::from_byte_array(rng.next_32_bytes());
        let vout = (rng.next_u64() % u32::MAX as u64) as u32;
        let outpoint = OutPoint { txid, vout };
        acc.spend(outpoint);
        outpoints.push(outpoint);
    }
    assert!(!acc.is_zero());
    for outpoint in outpoints {
        acc.add(outpoint);
    }
    assert!(acc.is_zero());
}
