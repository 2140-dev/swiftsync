# SwiftSync fast IBD

This binary implements a SwiftSync client that downloads blocks in parallel from multiple peers, references a hint file, and updates an accumulator. Once the client has reached the stop hash specified in the hint file, the accumulator state is reported as verified or false. For more information, read the [SwiftSync specification](https://gist.github.com/RubenSomsen/a61a37d14182ccd78760e477c78133cd).

You will need a `.hints` file locally to run this binary. There is one committed to by the repository as a `zip` file. You may uncompress it with the `unzip` tool.

```
sudo apt-get install unzip
```

```
unzip bitcoin.hints.zip
```

To build the Bitcoin kernel, you will need the following on Ubuntu:

```
sudo apt-get install build-essential cmake pkgconf python3 libevent-dev libboost-dev
```

For other systems, follow the Bitcoin Core documentation on how to install the requirements [here](https://github.com/bitcoin/bitcoin/tree/master/doc).

Finally, you will need Rust and cargo installed, you may download them from [here](https://www.rust-lang.org/tools/install).

To start fast IBD:

```
cargo run --bin ibd --release -- <args>
```

```
Arguments:
        --hintfile              The path to your `bitcoin.hints` file that will
                                be used for IBD. Default is `./bitcoin.hints`
        --blocks-dir            Optional directory to store the blocks. Used
                                only to measure performance.
        --network               The bitcoin network to operate on. Default `
                                bitcoin`. Options are `bitcoin` or `signet`
        --min-blocks-per-sec    The minimum rate a peer has to respond to block
                                requests.
        --tasks                 The number of tasks to download blocks. Default
                                is 64. Each task uses two OS threads.
        --ping-timeout          The time (seconds) a peer has to respond to a `
                                ping` message. Pings are sent aggressively
                                throughout IBD to find slow peers.
        --tcp-timeout           The maximum time (seconds) to establish a
                                connection
        --read-timeout          The maximum time (seconds) to read from a TCP
                                stream until the connection is killed.
        --write-timeout         The maximum time (seconds) to write to a TCP
                                stream until the connection is killed.
```

If IBD completes, or you experience a bug, you will need to remove the kernel directories from this repository to run the binary again: 

```
rm -rf blocks chainstate
```
