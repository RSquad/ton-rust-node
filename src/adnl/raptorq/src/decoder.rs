use crate::{
    base::{
        intermediate_tuple, partition, EncodingPacket, ObjectTransmissionInformation, PayloadId,
    },
    constraint_matrix::{enc_indices, generate_constraint_matrix},
    encoder::SPARSE_MATRIX_THRESHOLD,
    matrix::{BinaryMatrix, DenseBinaryMatrix},
    octet_matrix::DenseOctetMatrix,
    pi_solver::fused_inverse_mul_symbols,
    sparse_matrix::SparseBinaryMatrix,
    symbol::Symbol,
    systematic_constants::{
        calculate_p1, extended_source_block_symbols, num_hdpc_symbols, num_ldpc_symbols,
        num_lt_symbols, num_pi_symbols, systematic_index,
    },
    util::int_div_ceil,
};
#[cfg(not(feature = "std"))]
use alloc::{collections::BTreeSet as Set, vec::Vec};
#[cfg(not(feature = "std"))]
use core::iter;
#[cfg(feature = "serde_support")]
use serde::{Deserialize, Serialize};
#[cfg(feature = "std")]
use std::{collections::HashSet as Set, iter, vec::Vec};

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde_support", derive(Serialize, Deserialize))]
pub struct Decoder {
    config: ObjectTransmissionInformation,
    block_decoders: Vec<SourceBlockDecoder>,
    blocks: Vec<Option<Vec<u8>>>,
}

impl Decoder {
    pub fn new(config: ObjectTransmissionInformation) -> Decoder {
        let kt = int_div_ceil(config.transfer_length(), config.symbol_size() as u64);

        let (kl, ks, zl, zs) = partition(kt, config.source_blocks());

        let mut decoders = vec![];
        for i in 0..zl {
            decoders.push(SourceBlockDecoder::new2(
                i as u8,
                &config,
                u64::from(kl) * u64::from(config.symbol_size()),
            ));
        }

        for i in zl..(zl + zs) {
            decoders.push(SourceBlockDecoder::new2(
                i as u8,
                &config,
                u64::from(ks) * u64::from(config.symbol_size()),
            ));
        }

        Decoder { config, block_decoders: decoders, blocks: vec![None; (zl + zs) as usize] }
    }

    #[cfg(all(
        any(test, feature = "benchmarking"),
        not(any(feature = "python", feature = "wasm"))
    ))]
    pub fn set_sparse_threshold(&mut self, value: u32) {
        for block_decoder in self.block_decoders.iter_mut() {
            block_decoder.set_sparse_threshold(value);
        }
    }

    pub fn decode(&mut self, packet: EncodingPacket) -> Option<Vec<u8>> {
        let block_number = packet.payload_id.source_block_number() as usize;
        if self.blocks[block_number].is_none() {
            self.blocks[block_number] =
                self.block_decoders[block_number].decode(iter::once(packet));
        }
        for block in self.blocks.iter() {
            if block.is_none() {
                return None;
            }
        }

        let mut result = vec![];
        for block in self.blocks.iter().flatten() {
            result.extend(block);
        }

        result.truncate(self.config.transfer_length() as usize);
        Some(result)
    }

    #[cfg(not(any(feature = "python", feature = "wasm")))]
    pub fn add_new_packet(&mut self, packet: EncodingPacket) {
        let block_number = packet.payload_id.source_block_number() as usize;
        if self.blocks[block_number].is_none() {
            self.blocks[block_number] =
                self.block_decoders[block_number].decode(iter::once(packet));
        }
    }

    #[cfg(not(any(feature = "python", feature = "wasm")))]
    pub fn get_result(&self) -> Option<Vec<u8>> {
        for block in self.blocks.iter() {
            if block.is_none() {
                return None;
            }
        }

        let mut result = vec![];
        for block in self.blocks.iter().flatten() {
            result.extend(block);
        }
        result.truncate(self.config.transfer_length() as usize);
        Some(result)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde_support", derive(Serialize, Deserialize))]
pub struct SourceBlockDecoder {
    source_block_id: u8,
    symbol_size: u32,
    num_sub_blocks: u16,
    symbol_alignment: u8,
    source_block_symbols: u32,
    source_symbols: Vec<Option<Symbol>>,
    repair_packets: Vec<EncodingPacket>,
    received_source_symbols: u32,
    received_esi: Set<u32>,
    decoded: bool,
    sparse_threshold: u32,
}

impl SourceBlockDecoder {
    #[deprecated(
        since = "1.3.0",
        note = "Use the new2() function instead. In version 2.0, that function will replace this one"
    )]
    #[cfg(feature = "std")]
    pub fn new(source_block_id: u8, symbol_size: u32, block_length: u64) -> SourceBlockDecoder {
        let config = ObjectTransmissionInformation::new(0, symbol_size, 0, 1, 1);
        SourceBlockDecoder::new2(source_block_id, &config, block_length)
    }

    // TODO: rename this to new() in version 2.0
    pub fn new2(
        source_block_id: u8,
        config: &ObjectTransmissionInformation,
        block_length: u64,
    ) -> SourceBlockDecoder {
        let source_symbols = int_div_ceil(block_length, config.symbol_size() as u64);

        let mut received_esi = Set::new();
        for i in source_symbols..extended_source_block_symbols(source_symbols) {
            received_esi.insert(i);
        }
        SourceBlockDecoder {
            source_block_id,
            symbol_size: config.symbol_size(),
            num_sub_blocks: config.sub_blocks(),
            symbol_alignment: config.symbol_alignment(),
            source_block_symbols: source_symbols,
            source_symbols: vec![None; source_symbols as usize],
            repair_packets: vec![],
            received_source_symbols: 0,
            received_esi,
            decoded: false,
            sparse_threshold: SPARSE_MATRIX_THRESHOLD,
        }
    }

    #[cfg(any(test, feature = "benchmarking"))]
    pub fn set_sparse_threshold(&mut self, value: u32) {
        self.sparse_threshold = value;
    }

    fn unpack_sub_blocks(&self, result: &mut [u8], symbol: &Symbol, symbol_index: usize) {
        let (tl, ts, nl, ns) =
            partition(self.symbol_size / self.symbol_alignment as u32, self.num_sub_blocks);

        let mut symbol_offset = 0;
        let mut sub_block_offset = 0;
        for sub_block in 0..(nl + ns) {
            let bytes = if sub_block < nl {
                tl as usize * self.symbol_alignment as usize
            } else {
                ts as usize * self.symbol_alignment as usize
            };
            let start = sub_block_offset + bytes * symbol_index;
            result[start..start + bytes]
                .copy_from_slice(&symbol.as_bytes()[symbol_offset..symbol_offset + bytes]);
            symbol_offset += bytes;
            sub_block_offset += bytes * self.source_block_symbols as usize;
        }
    }

    fn try_pi_decode(
        &mut self,
        constraint_matrix: impl BinaryMatrix,
        hdpc_rows: DenseOctetMatrix,
        symbols: Vec<Symbol>,
    ) -> Option<Vec<u8>> {
        let intermediate_symbols = match fused_inverse_mul_symbols(
            constraint_matrix,
            hdpc_rows,
            symbols,
            self.source_block_symbols,
        ) {
            (None, _) => return None,
            (Some(s), _) => s,
        };

        let mut result = vec![0; self.symbol_size as usize * self.source_block_symbols as usize];
        let lt_symbols = num_lt_symbols(self.source_block_symbols);
        let pi_symbols = num_pi_symbols(self.source_block_symbols);
        let sys_index = systematic_index(self.source_block_symbols);
        let p1 = calculate_p1(self.source_block_symbols, pi_symbols);
        for i in 0..self.source_block_symbols as usize {
            if let Some(ref symbol) = self.source_symbols[i] {
                self.unpack_sub_blocks(&mut result, symbol, i);
            } else {
                let rebuilt = self.rebuild_source_symbol(
                    &intermediate_symbols,
                    i as u32,
                    lt_symbols,
                    pi_symbols,
                    sys_index,
                    p1,
                );
                self.unpack_sub_blocks(&mut result, &rebuilt, i);
            }
        }

        self.decoded = true;
        return Some(result);
    }

    pub fn decode<T: IntoIterator<Item = EncodingPacket>>(
        &mut self,
        packets: T,
    ) -> Option<Vec<u8>> {
        let num_extended_symbols = extended_source_block_symbols(self.source_block_symbols);
        for packet in packets {
            assert_eq!(self.source_block_id, packet.payload_id.source_block_number());

            let (payload_id, payload) = packet.split();
            let symbol_id = if payload_id.encoding_symbol_id() >= self.source_block_symbols {
                payload_id.encoding_symbol_id() + num_extended_symbols - self.source_block_symbols
            } else {
                payload_id.encoding_symbol_id()
            };
            if self.received_esi.insert(symbol_id) {
                if symbol_id >= num_extended_symbols {
                    // Repair symbol
                    self.repair_packets
                        .push(EncodingPacket::new(PayloadId::new(0, symbol_id), payload));
                } else {
                    // Check that this is not an extended symbol (which aren't explicitly sent)
                    assert!(symbol_id < self.source_block_symbols);
                    // Source symbol
                    self.source_symbols[symbol_id as usize] = Some(Symbol::new(payload));
                    self.received_source_symbols += 1;
                }
            }
        }

        if self.received_source_symbols == self.source_block_symbols {
            let mut result =
                vec![0; self.symbol_size as usize * self.source_block_symbols as usize];
            for (i, symbol) in self.source_symbols.iter().enumerate() {
                self.unpack_sub_blocks(&mut result, symbol.as_ref().unwrap(), i);
            }

            self.decoded = true;
            return Some(result);
        }

        if self.received_esi.len() as u32 >= num_extended_symbols {
            let s = num_ldpc_symbols(self.source_block_symbols) as usize;
            let h = num_hdpc_symbols(self.source_block_symbols) as usize;

            let mut encoded_indices = vec![];
            // See section 5.3.3.4.2. There are S + H zero symbols to start the D vector
            let mut d = vec![Symbol::zero(self.symbol_size as usize); s + h];
            for (i, source) in self.source_symbols.iter().enumerate() {
                if let Some(symbol) = source {
                    encoded_indices.push(i as u32);
                    d.push(symbol.clone());
                }
            }

            // Append the extended padding symbols
            for i in self.source_block_symbols..num_extended_symbols {
                encoded_indices.push(i);
                d.push(Symbol::zero(self.symbol_size as usize));
            }

            for repair_packet in self.repair_packets.iter() {
                encoded_indices.push(repair_packet.payload_id.encoding_symbol_id());
                d.push(Symbol::new(repair_packet.data.clone()));
            }

            if extended_source_block_symbols(self.source_block_symbols) >= self.sparse_threshold {
                let (constraint_matrix, hdpc) = generate_constraint_matrix::<SparseBinaryMatrix>(
                    self.source_block_symbols,
                    &encoded_indices,
                );
                return self.try_pi_decode(constraint_matrix, hdpc, d);
            } else {
                let (constraint_matrix, hdpc) = generate_constraint_matrix::<DenseBinaryMatrix>(
                    self.source_block_symbols,
                    &encoded_indices,
                );
                return self.try_pi_decode(constraint_matrix, hdpc, d);
            }
        }
        None
    }

    fn rebuild_source_symbol(
        &self,
        intermediate_symbols: &[Symbol],
        source_symbol_id: u32,
        lt_symbols: u32,
        pi_symbols: u32,
        sys_index: u32,
        p1: u32,
    ) -> Symbol {
        let mut rebuilt = Symbol::zero(self.symbol_size as usize);
        let tuple = intermediate_tuple(source_symbol_id, lt_symbols, sys_index, p1);

        for i in enc_indices(tuple, lt_symbols, pi_symbols, p1) {
            rebuilt += &intermediate_symbols[i];
        }
        rebuilt
    }
}
