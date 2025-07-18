# SwiftSync

:warning::construction: This project is still under construction and expected to change significantly. Use at your own risk :construction::warning:

This repository is a collection of crates related to a SwiftSync node implementation. Some crates will be SwiftSync-specific, while others may have broader use cases.

`accumulator`: A hash-based SwiftSync accumulator used to add and subtrack elements from a set.
`p2p`: Utilities for creating and facilitating Bitcoin peer-to-peer connections.
`peers`: Tools to find Bitcoin peers
`utxo_verifier`: A binary application that uses each of the above crates to verify a UTXO snapshot is valid.

