# SwiftSync fast IBD

This binary implements a SwiftSync client that downloads blocks in parallel from multiple peers, references a hint file, and updates an accumulator. Once the client has reached the stop hash specified in the hint file, the accumulator state is reported as verified or false. For more information, read the [SwiftSync specification](https://gist.github.com/RubenSomsen/a61a37d14182ccd78760e477c78133cd).

You will need a `.hints` file locally to run this binary. See the `hintfile` create in this workspace to generate one from Bitcoin Core.

To start fast IBD:
```
cargo run --bin ibd --release -- <args>
```

```
Arguments:

        --hintfile              The path to your `bitcoin.hints` file that will
                                be used for IBD. Default is `./bitcoin.hints
        --blocks-dir            Directory where you would like to store the
                                bitcoin blocks. Default `./blockfiles`
        --network               The bitcoin network to operate on. Default `
                                bitcoin`. Options are `bitcoin` or `signet`
        --ping-timeout          The time (seconds) a peer has to respond to a `
                                ping` message. Pings are sent aggressively
                                throughout IBD to find slow peers.
        --tcp-timeout           The maximum time (seconds) to establish a
                                connection
        --read-timeout          The maximum time (seconds) to read from a TCP
                                stream until the connection is killed.
        --write-timeout         The maximum time (seconds) to write to a TCP
                                stream until the connection is killed.
        --min-blocks-per-sec    The minimum rate a peer has to respond to block
                                requests.
        --tasks                 The number of tasks to download blocks. Default
                                is 64. Each task uses two OS threads.
```
