# Phase One

First step is to make something with some nice "shock-value", benchmarks, etc. With no changes to the p2p layer, only the assume-valid version of SwiftSync is possible. This restricts the feature set to:

- [x] Use kernel to download the block header chain to the assume-valid height
- [x] Build a hintfile that clients can parse
- [x] Use a hintfile to determine if one should update an accumulator or add a UTXO
- [x] Download blocks in parallel
- [ ] Add coins to Bitcoin Core (kernel)
- [ ] Start `bitcoind` in pruned mode

To complete this we need(ed) from kernel:
- [x] Process new block headers
- [x] `HaveCoin` to generate the hintfile
- [ ] `AddCoin` to bypass `ProcessBlock`

# Phase Two

Next, some changes are required to move beyond assume-valid.

Patches to Bitcoin Core:
- [ ] Send undo-data over the p2p layer for parallel validation (and batch validation)
- [ ] Create a hintfile from Bitcoin Core

Additions to SwiftSync:
- [ ] Validate blocks
- [ ] Write blocks to kernel

Additions to kernel:
- [ ] Write blocks without `ProcessBlock`

