use std::sync::mpsc;

use accumulator::Accumulator;
use bitcoin::{Network, OutPoint, Txid};

use crate::{AccumulatorUpdate, NETWORK};

pub fn update_accumulator_from_blocks(
    receiver: mpsc::Receiver<Vec<AccumulatorUpdate>>,
) -> Accumulator {
    let mut acc = Accumulator::new();
    // These outpoints will show up twice, but can only be spent once
    //
    if matches!(NETWORK, Network::Bitcoin) {
        let coinbase_one = crate::DUP_COINBASE_ONE.parse::<Txid>().unwrap();
        let coinbase_two = crate::DUP_COINBASE_TWO.parse::<Txid>().unwrap();
        acc.spend(OutPoint {
            txid: coinbase_one,
            vout: 0,
        });
        acc.spend(OutPoint {
            txid: coinbase_two,
            vout: 0,
        });
    }
    while let Ok(values) = receiver.recv() {
        for value in values {
            match value {
                AccumulatorUpdate::Spent(outpoint) => acc.spend_hashed_outpoint(outpoint),
                AccumulatorUpdate::Created(txout) => acc.add_hashed_outpoint(txout),
            }
        }
    }
    acc
}
