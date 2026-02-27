# SwiftSync

:construction: This is a research project not intended for real use. :construction:

This repository is a collection of crates related to a _SwiftSync_ node implementation. _SwiftSync_ is a protocol that allows nearly-stateless, parallelizable Bitcoin initial block download without adding additional cryptographic assumptions. You may read the [initial writeup here](https://gist.github.com/RubenSomsen/a61a37d14182ccd78760e477c78133cd).

## Executables

See the `node/README.md` to run an initial block download using _SwiftSync_.

## Crates

- `aggregate`: A hash-based data structure used to add and subtrack elements from a set.
- `node`: Perform fast IBD using a SwiftSync hints file.
