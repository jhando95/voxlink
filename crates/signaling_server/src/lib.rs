//! Minimal library surface for criterion benches.
//!
//! The server is a binary (see main.rs); this lib.rs exists only so that
//! criterion benches can reach internal types like `Histogram`. Runtime
//! code continues to use the `mod` tree in main.rs.

pub mod histogram;
