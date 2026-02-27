# SwiftSync Accumulator

This crate defines a simple accumulator that may add and spend `OutPoint` from a set. If the `OutPoint` are add and spent from the accumulator an equivalent number of times, the accumulator state is zero. Internally, the accumulator is represented as two `u128` integers. Hashing elements is far more computationally expensive than adding and subtracing from `u128` integers, so hashes may be pre-computed with functions in this crate.
