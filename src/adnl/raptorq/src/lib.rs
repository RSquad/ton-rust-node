#![allow(clippy::needless_return, clippy::unreadable_literal)]
#![no_std]

#[cfg(not(feature = "std"))]
#[macro_use]
extern crate alloc;

#[cfg(not(feature = "std"))]
extern crate core;

#[cfg(feature = "std")]
#[macro_use]
extern crate std;

mod arraymap;
mod base;
mod constraint_matrix;
mod decoder;
mod encoder;
mod gf2;
mod graph;
mod iterators;
mod matrix;
mod octet;
mod octet_matrix;
mod octets;
mod operation_vector;
mod pi_solver;
#[cfg(feature = "python")]
mod python;
mod rng;
mod sparse_matrix;
mod sparse_vec;
mod symbol;
mod systematic_constants;
mod util;
#[cfg(feature = "wasm")]
mod wasm;

#[cfg(feature = "benchmarking")]
pub use crate::constraint_matrix::generate_constraint_matrix;
#[cfg(not(any(feature = "python", feature = "wasm")))]
pub use crate::decoder::Decoder;
#[cfg(not(any(feature = "python", feature = "wasm")))]
pub use crate::encoder::Encoder;
#[cfg(feature = "benchmarking")]
pub use crate::matrix::BinaryMatrix;
#[cfg(feature = "benchmarking")]
pub use crate::matrix::DenseBinaryMatrix;
#[cfg(feature = "benchmarking")]
pub use crate::octet::Octet;
#[cfg(feature = "benchmarking")]
pub use crate::pi_solver::IntermediateSymbolDecoder;
#[cfg(feature = "python")]
pub use crate::python::raptorq;
#[cfg(feature = "python")]
pub use crate::python::Decoder;
#[cfg(feature = "python")]
pub use crate::python::Encoder;
#[cfg(feature = "benchmarking")]
pub use crate::sparse_matrix::SparseBinaryMatrix;
#[cfg(feature = "benchmarking")]
pub use crate::symbol::Symbol;
#[cfg(feature = "wasm")]
pub use crate::wasm::Decoder as WasmDecoder;
#[cfg(feature = "wasm")]
pub use crate::wasm::Encoder as WasmEncoder;
pub use crate::{
    base::{partition, EncodingPacket, ObjectTransmissionInformation, PayloadId},
    decoder::SourceBlockDecoder,
    encoder::{
        calculate_block_offsets, EncoderBuilder, SourceBlockEncoder, SourceBlockEncodingPlan,
    },
    systematic_constants::extended_source_block_symbols,
};
