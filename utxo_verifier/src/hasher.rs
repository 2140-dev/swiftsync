use std::sync::mpsc;

use accumulator::Accumulator;

use crate::AccumulatorUpdate;

pub fn update_accumulator_from_blocks(
    receiver: mpsc::Receiver<Vec<AccumulatorUpdate>>,
) -> Accumulator {
    let mut acc = Accumulator::new();
    while let Ok(values) = receiver.recv() {
        for value in values {
            match value {
                AccumulatorUpdate::Spent(outpoint) => acc.spend(outpoint),
                AccumulatorUpdate::Created(txout) => acc.add(txout),
            }
        }
    }
    acc
}
