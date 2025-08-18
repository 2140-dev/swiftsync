# SwiftSync

:warning::construction: This project is still under construction and expected to change significantly. Use at your own risk :construction::warning:

This repository is a collection of crates related to a SwiftSync node implementation. Some crates will be SwiftSync-specific, while others may have broader use cases.

- `accumulator`: A hash-based SwiftSync accumulator used to add and subtrack elements from a set.
- `hintfile`: Generate a SwiftSync hintfile as the role of a server.
- `node`: Perform fast IBD using a SwiftSync hints file.
- `network`: Tools to find Bitcoin peers.
