# UTXO Snapshot Verifier

The UTXO set at a particular height in the blockchain may be verified by computing an accumulator state generated from the snapshot and the accumulator state generated from downloading the block history. This program accepts a path to a sqlite file representing the OutPoints in the UTXO set and downloads blocks in parallel from many peers. The result of the program is either successful in the case the accumulators match, or panic if there is a mis-match. The snapshot height used is 880_000 on Bitcoin and is 160_000 on Signet.

## SQLite File Generation

The sqlite file for Signet is in the `/contrib` folder of this repository. Skip to the next section to run the Signet example. To generate the sqlite file required to run this verifier on Bitcoin, you will need a `utxos.dat` file representing the UTXO state at height 880_000. This may be found on https://utxo.download. Be aware you will need at least 32GB of storage at the time of writing.

```
curl https://utxo.download/mainnet-880000.dat --output utxos.dat
```

To convert this snapshot to sqlite, in this directory:

```
python utxos_to_sqlite.py /path/to/utxos.dat /path/to/outpoints.sqlite
```

## Running the Verifier

To run the verifier on Signet:
```
cargo run ../contrib/signet_outpoints.sqlite --release
```
