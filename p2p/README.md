# Domain Specific Bitcoin Peer-to-Peer Crate

This crate is intended to facilitate connections between two bitcoin nodes. Many messages exchanged over the bitcoin network are involved in version/feature negotiation. These message exchanges are abstracted away by the types of this crate, so users can focus on the messages that are important for the implementation. When reading messages, some simple validation is performed to omit spam or malformed messages. Writing invalid messages is also made difficult. Through the use of extension traits, one can read/write protocol messages directly to a TCP stream.

## Example

Try the example, which fetches a block and updates a SwiftSync accumulator:
```
cargo run --example update_accumulator --release
```

