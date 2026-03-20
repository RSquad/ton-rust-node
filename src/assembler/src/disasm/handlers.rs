/*
 * Copyright (C) 2019-2024 EverX. All Rights Reserved.
 * Modifications Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This file has been modified from its original version.
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use super::{
    loader::*,
    types::{Instruction, Quiet, Signaling},
};
use ton_block::{Result, SliceData};

pub(super) type LoadHandler = fn(&mut Loader, &mut SliceData) -> Result<Instruction>;

#[derive(Clone, Copy)]
enum Handler {
    Direct(LoadHandler),
    Subset(usize),
}

pub struct Handlers {
    directs: [Handler; 256],
    subsets: Vec<Handlers>,
}

// adapted from vm/src/executor/engine/handlers.rs
impl Handlers {
    pub fn new() -> Handlers {
        Handlers { directs: [Handler::Direct(Loader::unknown); 256], subsets: Vec::new() }
    }

    pub fn new_code_page_0() -> Handlers {
        let mut handlers = Handlers::new();
        handlers
            .add_code_page_0_part_stack()
            .add_code_page_0_tuple()
            .add_code_page_0_part_constant()
            .add_code_page_0_arithmetic()
            .add_code_page_0_comparsion()
            .add_code_page_0_cell()
            .add_code_page_0_control_flow()
            .add_code_page_0_exceptions()
            .add_code_page_0_dictionaries()
            .add_code_page_0_gas_rand_config()
            .add_code_page_0_blockchain()
            .add_code_page_0_crypto()
            .add_code_page_0_debug()
            .add_subset(
                0xFF,
                Handlers::new()
                    .set_range(0x00..0xF0, Loader::setcp)
                    .set(0xF0, Loader::setcpx)
                    .set_range(0xF1..0xFF, Loader::setcp)
                    .set(0xFF, Loader::setcp),
            );
        handlers
    }

    fn add_code_page_0_part_stack(&mut self) -> &mut Handlers {
        self.set(0x00, Loader::nop)
            .set_range(0x01..0x10, Loader::xchg_simple)
            .set(0x10, Loader::xchg_std)
            .set(0x11, Loader::xchg_long)
            .set_range(0x12..0x20, Loader::xchg_simple)
            .set_range(0x20..0x30, Loader::push_simple)
            .set_range(0x30..0x40, Loader::pop_simple)
            .set_range(0x40..0x50, Loader::xchg3)
            .set(0x50, Loader::xchg2)
            .set(0x51, Loader::xcpu)
            .set(0x52, Loader::puxc)
            .set(0x53, Loader::push2)
            .add_subset(
                0x54,
                Handlers::new()
                    .set_range(0x00..0x10, Loader::xchg3)
                    .set_range(0x10..0x20, Loader::xc2pu)
                    .set_range(0x20..0x30, Loader::xcpuxc)
                    .set_range(0x30..0x40, Loader::xcpu2)
                    .set_range(0x40..0x50, Loader::puxc2)
                    .set_range(0x50..0x60, Loader::puxcpu)
                    .set_range(0x60..0x70, Loader::pu2xc)
                    .set_range(0x70..0x80, Loader::push3),
            )
            .set(0x55, Loader::blkswap)
            .set(0x56, Loader::push)
            .set(0x57, Loader::pop)
            .set(0x58, Loader::rot)
            .set(0x59, Loader::rotrev)
            .set(0x5A, Loader::swap2)
            .set(0x5B, Loader::drop2)
            .set(0x5C, Loader::dup2)
            .set(0x5D, Loader::over2)
            .set(0x5E, Loader::reverse)
            .add_subset(
                0x5F,
                Handlers::new()
                    .set_range(0x00..0x10, Loader::blkdrop)
                    .set_range(0x10..0xFF, Loader::blkpush)
                    .set(0xFF, Loader::blkpush),
            )
            .set(0x60, Loader::pick)
            .set(0x61, Loader::rollx)
            .set(0x62, Loader::rollrevx)
            .set(0x63, Loader::blkswx)
            .set(0x64, Loader::revx)
            .set(0x65, Loader::dropx)
            .set(0x66, Loader::tuck)
            .set(0x67, Loader::xchgx)
            .set(0x68, Loader::depth)
            .set(0x69, Loader::chkdepth)
            .set(0x6A, Loader::onlytopx)
            .set(0x6B, Loader::onlyx)
            .add_subset(
                0x6C,
                Handlers::new().set_range(0x10..0xFF, Loader::blkdrop2).set(0xFF, Loader::blkdrop2),
            )
    }

    fn add_code_page_0_tuple(&mut self) -> &mut Handlers {
        self.set(0x6D, Loader::null).set(0x6E, Loader::isnull).add_subset(
            0x6F,
            Handlers::new()
                .set_range(0x00..0x10, Loader::tuple_create)
                .set_range(0x10..0x20, Loader::tuple_index)
                .set_range(0x20..0x30, Loader::tuple_un)
                .set_range(0x30..0x40, Loader::tuple_unpackfirst)
                .set_range(0x40..0x50, Loader::tuple_explode)
                .set_range(0x50..0x60, Loader::tuple_setindex)
                .set_range(0x60..0x70, Loader::tuple_index_quiet)
                .set_range(0x70..0x80, Loader::tuple_setindex_quiet)
                .set(0x80, Loader::tuple_createvar)
                .set(0x81, Loader::tuple_indexvar)
                .set(0x82, Loader::tuple_untuplevar)
                .set(0x83, Loader::tuple_unpackfirstvar)
                .set(0x84, Loader::tuple_explodevar)
                .set(0x85, Loader::tuple_setindexvar)
                .set(0x86, Loader::tuple_indexvar_quiet)
                .set(0x87, Loader::tuple_setindexvar_quiet)
                .set(0x88, Loader::tuple_len)
                .set(0x89, Loader::tuple_len_quiet)
                .set(0x8A, Loader::istuple)
                .set(0x8B, Loader::tuple_last)
                .set(0x8C, Loader::tuple_push)
                .set(0x8D, Loader::tuple_pop)
                .set(0xA0, Loader::nullswapif)
                .set(0xA1, Loader::nullswapifnot)
                .set(0xA2, Loader::nullrotrif)
                .set(0xA3, Loader::nullrotrifnot)
                .set(0xA4, Loader::nullswapif2)
                .set(0xA5, Loader::nullswapifnot2)
                .set(0xA6, Loader::nullrotrif2)
                .set(0xA7, Loader::nullrotrifnot2)
                .set_range(0xB0..0xC0, Loader::tuple_index2)
                .set_range(0xC0..0xFF, Loader::tuple_index3)
                .set(0xFF, Loader::tuple_index3),
        )
    }

    fn add_code_page_0_part_constant(&mut self) -> &mut Handlers {
        self.set_range(0x70..0x82, Loader::pushint)
            .set(0x82, Loader::pushint_big)
            .add_subset(
                0x83,
                Handlers::new().set_range(0x00..0xFF, Loader::pushpow2).set(0xFF, Loader::pushnan),
            )
            .set(0x84, Loader::pushpow2dec)
            .set(0x85, Loader::pushnegpow2)
            .set(0x88, Loader::pushref)
            .set(0x89, Loader::pushrefslice)
            .set(0x8A, Loader::pushrefcont)
            .set(0x8B, Loader::pushslice_short)
            .set(0x8C, Loader::pushslice_mid)
            .set(0x8D, Loader::pushslice_long)
            .set_range(0x8E..0x90, Loader::pushcont_long)
            .set_range(0x90..0xA0, Loader::pushcont_short)
    }

    fn add_code_page_0_arithmetic(&mut self) -> &mut Handlers {
        self.set(0xA0, Loader::add::<Signaling>)
            .set(0xA1, Loader::sub::<Signaling>)
            .set(0xA2, Loader::subr::<Signaling>)
            .set(0xA3, Loader::negate::<Signaling>)
            .set(0xA4, Loader::inc::<Signaling>)
            .set(0xA5, Loader::dec::<Signaling>)
            .set(0xA6, Loader::addconst::<Signaling>)
            .set(0xA7, Loader::mulconst::<Signaling>)
            .set(0xA8, Loader::mul::<Signaling>)
            .set(0xA9, Loader::divmod::<Signaling>)
            .set(0xAA, Loader::lshift::<Signaling>)
            .set(0xAB, Loader::rshift::<Signaling>)
            .set(0xAC, Loader::lshift::<Signaling>)
            .set(0xAD, Loader::rshift::<Signaling>)
            .set(0xAE, Loader::pow2::<Signaling>)
            .set(0xB0, Loader::and::<Signaling>)
            .set(0xB1, Loader::or::<Signaling>)
            .set(0xB2, Loader::xor::<Signaling>)
            .set(0xB3, Loader::not::<Signaling>)
            .set(0xB4, Loader::fits::<Signaling>)
            .set(0xB5, Loader::ufits::<Signaling>)
            .add_subset(
                0xB6,
                Handlers::new()
                    .set(0x00, Loader::fitsx::<Signaling>)
                    .set(0x01, Loader::ufitsx::<Signaling>)
                    .set(0x02, Loader::bitsize::<Signaling>)
                    .set(0x03, Loader::ubitsize::<Signaling>)
                    .set(0x08, Loader::min::<Signaling>)
                    .set(0x09, Loader::max::<Signaling>)
                    .set(0x0A, Loader::minmax::<Signaling>)
                    .set(0x0B, Loader::abs::<Signaling>),
            )
            .add_subset(
                0xB7,
                Handlers::new()
                    .set(0xA0, Loader::add::<Quiet>)
                    .set(0xA1, Loader::sub::<Quiet>)
                    .set(0xA2, Loader::subr::<Quiet>)
                    .set(0xA3, Loader::negate::<Quiet>)
                    .set(0xA4, Loader::inc::<Quiet>)
                    .set(0xA5, Loader::dec::<Quiet>)
                    .set(0xA6, Loader::addconst::<Quiet>)
                    .set(0xA7, Loader::mulconst::<Quiet>)
                    .set(0xA8, Loader::mul::<Quiet>)
                    .set(0xA9, Loader::divmod::<Quiet>)
                    .set(0xAA, Loader::lshift::<Quiet>)
                    .set(0xAB, Loader::rshift::<Quiet>)
                    .set(0xAC, Loader::lshift::<Quiet>)
                    .set(0xAD, Loader::rshift::<Quiet>)
                    .set(0xAE, Loader::pow2::<Quiet>)
                    .set(0xB0, Loader::and::<Quiet>)
                    .set(0xB1, Loader::or::<Quiet>)
                    .set(0xB2, Loader::xor::<Quiet>)
                    .set(0xB3, Loader::not::<Quiet>)
                    .set(0xB4, Loader::fits::<Quiet>)
                    .set(0xB5, Loader::ufits::<Quiet>)
                    .add_subset(
                        0xB6,
                        Handlers::new()
                            .set(0x00, Loader::fitsx::<Quiet>)
                            .set(0x01, Loader::ufitsx::<Quiet>)
                            .set(0x02, Loader::bitsize::<Quiet>)
                            .set(0x03, Loader::ubitsize::<Quiet>)
                            .set(0x08, Loader::min::<Quiet>)
                            .set(0x09, Loader::max::<Quiet>)
                            .set(0x0A, Loader::minmax::<Quiet>)
                            .set(0x0B, Loader::abs::<Quiet>),
                    )
                    .set(0xB8, Loader::sgn::<Quiet>)
                    .set(0xB9, Loader::less::<Quiet>)
                    .set(0xBA, Loader::equal::<Quiet>)
                    .set(0xBB, Loader::leq::<Quiet>)
                    .set(0xBC, Loader::greater::<Quiet>)
                    .set(0xBD, Loader::neq::<Quiet>)
                    .set(0xBE, Loader::geq::<Quiet>)
                    .set(0xBF, Loader::cmp::<Quiet>)
                    .set(0xC0, Loader::eqint::<Quiet>)
                    .set(0xC1, Loader::lessint::<Quiet>)
                    .set(0xC2, Loader::gtint::<Quiet>)
                    .set(0xC3, Loader::neqint::<Quiet>),
            )
    }

    fn add_code_page_0_comparsion(&mut self) -> &mut Handlers {
        self.set(0xB8, Loader::sgn::<Signaling>)
            .set(0xB9, Loader::less::<Signaling>)
            .set(0xBA, Loader::equal::<Signaling>)
            .set(0xBB, Loader::leq::<Signaling>)
            .set(0xBC, Loader::greater::<Signaling>)
            .set(0xBD, Loader::neq::<Signaling>)
            .set(0xBE, Loader::geq::<Signaling>)
            .set(0xBF, Loader::cmp::<Signaling>)
            .set(0xC0, Loader::eqint::<Signaling>)
            .set(0xC1, Loader::lessint::<Signaling>)
            .set(0xC2, Loader::gtint::<Signaling>)
            .set(0xC3, Loader::neqint::<Signaling>)
            .set(0xC4, Loader::isnan)
            .set(0xC5, Loader::chknan)
            .add_subset(
                0xC7,
                Handlers::new()
                    .set(0x00, Loader::sempty)
                    .set(0x01, Loader::sdempty)
                    .set(0x02, Loader::srempty)
                    .set(0x03, Loader::sdfirst)
                    .set(0x04, Loader::sdlexcmp)
                    .set(0x05, Loader::sdeq)
                    .set(0x08, Loader::sdpfx)
                    .set(0x09, Loader::sdpfxrev)
                    .set(0x0A, Loader::sdppfx)
                    .set(0x0B, Loader::sdppfxrev)
                    .set(0x0C, Loader::sdsfx)
                    .set(0x0D, Loader::sdsfxrev)
                    .set(0x0E, Loader::sdpsfx)
                    .set(0x0F, Loader::sdpsfxrev)
                    .set(0x10, Loader::sdcntlead0)
                    .set(0x11, Loader::sdcntlead1)
                    .set(0x12, Loader::sdcnttrail0)
                    .set(0x13, Loader::sdcnttrail1),
            )
    }

    fn add_code_page_0_cell(&mut self) -> &mut Handlers {
        self.set(0xC8, Loader::newc)
            .set(0xC9, Loader::endc)
            .set(0xCA, Loader::sti)
            .set(0xCB, Loader::stu)
            .set(0xCC, Loader::stref)
            .set(0xCD, Loader::endcst)
            .set(0xCE, Loader::stslice)
            .add_subset(
                0xCF,
                Handlers::new()
                    .set(0x00, Loader::stix)
                    .set(0x01, Loader::stux)
                    .set(0x02, Loader::stixr)
                    .set(0x03, Loader::stuxr)
                    .set(0x04, Loader::stixq)
                    .set(0x05, Loader::stuxq)
                    .set(0x06, Loader::stixrq)
                    .set(0x07, Loader::stuxrq)
                    .set(0x08, Loader::sti)
                    .set(0x09, Loader::stu)
                    .set(0x0A, Loader::stir)
                    .set(0x0B, Loader::stur)
                    .set(0x0C, Loader::stiq)
                    .set(0x0D, Loader::stuq)
                    .set(0x0E, Loader::stirq)
                    .set(0x0F, Loader::sturq)
                    .set(0x10, Loader::stref)
                    .set(0x11, Loader::stbref)
                    .set(0x12, Loader::stslice)
                    .set(0x13, Loader::stb)
                    .set(0x14, Loader::strefr)
                    .set(0x15, Loader::endcst)
                    .set(0x16, Loader::stslicer)
                    .set(0x17, Loader::stbr)
                    .set(0x18, Loader::strefq)
                    .set(0x19, Loader::stbrefq)
                    .set(0x1A, Loader::stsliceq)
                    .set(0x1B, Loader::stbq)
                    .set(0x1C, Loader::strefrq)
                    .set(0x1D, Loader::stbrefrq)
                    .set(0x1E, Loader::stslicerq)
                    .set(0x1F, Loader::stbrq)
                    .set(0x20, Loader::strefconst)
                    .set(0x21, Loader::stref2const)
                    .set(0x23, Loader::endxc)
                    .set(0x28, Loader::stile4)
                    .set(0x29, Loader::stule4)
                    .set(0x2A, Loader::stile8)
                    .set(0x2B, Loader::stule8)
                    .set(0x30, Loader::bdepth)
                    .set(0x31, Loader::bbits)
                    .set(0x32, Loader::brefs)
                    .set(0x33, Loader::bbitrefs)
                    .set(0x35, Loader::brembits)
                    .set(0x36, Loader::bremrefs)
                    .set(0x37, Loader::brembitrefs)
                    .set(0x38, Loader::bchkbits_short)
                    .set(0x39, Loader::bchkbits_long)
                    .set(0x3A, Loader::bchkrefs)
                    .set(0x3B, Loader::bchkbitrefs)
                    .set(0x3C, Loader::bchkbitsq_short)
                    .set(0x3D, Loader::bchkbitsq_long)
                    .set(0x3E, Loader::bchkrefsq)
                    .set(0x3F, Loader::bchkbitrefsq)
                    .set(0x40, Loader::stzeroes)
                    .set(0x41, Loader::stones)
                    .set(0x42, Loader::stsame)
                    .set(0x50, Loader::btos)
                    .set_range(0x80..0xFF, Loader::stsliceconst)
                    .set(0xFF, Loader::stsliceconst),
            )
            .set(0xD0, Loader::ctos)
            .set(0xD1, Loader::ends)
            .set(0xD2, Loader::ldi)
            .set(0xD3, Loader::ldu)
            .set(0xD4, Loader::ldref)
            .set(0xD5, Loader::ldrefrtos)
            .set(0xD6, Loader::ldslice)
            .add_subset(
                0xD7,
                Handlers::new()
                    .set(0x00, Loader::ldix)
                    .set(0x01, Loader::ldux)
                    .set(0x02, Loader::pldix)
                    .set(0x03, Loader::pldux)
                    .set(0x04, Loader::ldixq)
                    .set(0x05, Loader::lduxq)
                    .set(0x06, Loader::pldixq)
                    .set(0x07, Loader::plduxq)
                    .set(0x08, Loader::ldi)
                    .set(0x09, Loader::ldu)
                    .set(0x0A, Loader::pldi)
                    .set(0x0B, Loader::pldu)
                    .set(0x0C, Loader::ldiq)
                    .set(0x0D, Loader::lduq)
                    .set(0x0E, Loader::pldiq)
                    .set(0x0F, Loader::plduq)
                    .set_range(0x10..0x18, Loader::plduz)
                    .set(0x18, Loader::ldslicex)
                    .set(0x19, Loader::pldslicex)
                    .set(0x1A, Loader::ldslicexq)
                    .set(0x1B, Loader::pldslicexq)
                    .set(0x1C, Loader::ldslice)
                    .set(0x1D, Loader::pldslice)
                    .set(0x1E, Loader::ldsliceq)
                    .set(0x1F, Loader::pldsliceq)
                    .set(0x20, Loader::pldslicex)
                    .set(0x21, Loader::sdskipfirst)
                    .set(0x22, Loader::sdcutlast)
                    .set(0x23, Loader::sdskiplast)
                    .set(0x24, Loader::sdsubstr)
                    .set(0x26, Loader::sdbeginsx)
                    .set(0x27, Loader::sdbeginsxq)
                    .set_range(0x28..0x2C, Loader::sdbegins)
                    .set_range(0x2C..0x30, Loader::sdbeginsq)
                    .set(0x30, Loader::scutfirst)
                    .set(0x31, Loader::sskipfirst)
                    .set(0x32, Loader::scutlast)
                    .set(0x33, Loader::sskiplast)
                    .set(0x34, Loader::subslice)
                    .set(0x36, Loader::split)
                    .set(0x37, Loader::splitq)
                    .set(0x39, Loader::xctos)
                    .set(0x3A, Loader::xload)
                    .set(0x3B, Loader::xloadq)
                    .set(0x41, Loader::schkbits)
                    .set(0x42, Loader::schkrefs)
                    .set(0x43, Loader::schkbitrefs)
                    .set(0x45, Loader::schkbitsq)
                    .set(0x46, Loader::schkrefsq)
                    .set(0x47, Loader::schkbitrefsq)
                    .set(0x48, Loader::pldrefvar)
                    .set(0x49, Loader::sbits)
                    .set(0x4A, Loader::srefs)
                    .set(0x4B, Loader::sbitrefs)
                    .set(0x4C, Loader::pldref)
                    .set_range(0x4D..0x50, Loader::pldrefidx)
                    .set(0x50, Loader::ldile4)
                    .set(0x51, Loader::ldule4)
                    .set(0x52, Loader::ldile8)
                    .set(0x53, Loader::ldule8)
                    .set(0x54, Loader::pldile4)
                    .set(0x55, Loader::pldule4)
                    .set(0x56, Loader::pldile8)
                    .set(0x57, Loader::pldule8)
                    .set(0x58, Loader::ldile4q)
                    .set(0x59, Loader::ldule4q)
                    .set(0x5A, Loader::ldile8q)
                    .set(0x5B, Loader::ldule8q)
                    .set(0x5C, Loader::pldile4q)
                    .set(0x5D, Loader::pldule4q)
                    .set(0x5E, Loader::pldile8q)
                    .set(0x5F, Loader::pldule8q)
                    .set(0x60, Loader::ldzeroes)
                    .set(0x61, Loader::ldones)
                    .set(0x62, Loader::ldsame)
                    .set(0x64, Loader::sdepth)
                    .set(0x65, Loader::cdepth)
                    .set(0x66, Loader::clevel)
                    .set(0x67, Loader::clevel_mask)
                    .set_range(0x68..0x6C, Loader::chashi)
                    .set_range(0x6C..0x70, Loader::cdepthi)
                    .set(0x70, Loader::chashix)
                    .set(0x71, Loader::cdepthix),
            )
    }

    fn add_code_page_0_control_flow(&mut self) -> &mut Handlers {
        self.set(0xD8, Loader::callx)
            .set(0xD9, Loader::jmpx)
            .set(0xDA, Loader::callxargs)
            .add_subset(
                0xDB,
                Handlers::new()
                    .set_range(0x00..0x10, Loader::callxargs)
                    .set_range(0x10..0x20, Loader::jmpxargs)
                    .set_range(0x20..0x30, Loader::retargs)
                    .set(0x30, Loader::ret)
                    .set(0x31, Loader::retalt)
                    .set(0x32, Loader::retbool)
                    .set(0x34, Loader::callcc)
                    .set(0x35, Loader::jmpxdata)
                    .set(0x36, Loader::callccargs)
                    .set(0x38, Loader::callxva)
                    .set(0x39, Loader::retva)
                    .set(0x3A, Loader::jmpxva)
                    .set(0x3B, Loader::callccva)
                    .set(0x3C, Loader::callref)
                    .set(0x3D, Loader::jmpref)
                    .set(0x3E, Loader::jmprefdata)
                    .set(0x3F, Loader::retdata),
            )
            .set(0xDE, Loader::if_)
            .set(0xDC, Loader::ifret)
            .set(0xDD, Loader::ifnotret)
            .set(0xDF, Loader::ifnot)
            .set(0xE0, Loader::ifjmp)
            .set(0xE1, Loader::ifnotjmp)
            .set(0xE2, Loader::ifelse)
            .add_subset(
                0xE3,
                Handlers::new()
                    .set(0x00, Loader::ifref)
                    .set(0x01, Loader::ifnotref)
                    .set(0x02, Loader::ifjmpref)
                    .set(0x03, Loader::ifnotjmpref)
                    .set(0x04, Loader::condsel)
                    .set(0x05, Loader::condselchk)
                    .set(0x08, Loader::ifretalt)
                    .set(0x09, Loader::ifnotretalt)
                    .set(0x0D, Loader::ifrefelse)
                    .set(0x0E, Loader::ifelseref)
                    .set(0x0F, Loader::ifrefelseref)
                    .set(0x14, Loader::repeat_break)
                    .set(0x15, Loader::repeatend_break)
                    .set(0x16, Loader::until_break)
                    .set(0x17, Loader::untilend_break)
                    .set(0x18, Loader::while_break)
                    .set(0x19, Loader::whileend_break)
                    .set(0x1A, Loader::again_break)
                    .set(0x1B, Loader::againend_break)
                    .set_range(0x80..0xA0, Loader::ifbitjmp)
                    .set_range(0xA0..0xC0, Loader::ifnbitjmp)
                    .set_range(0xC0..0xE0, Loader::ifbitjmpref)
                    .set_range(0xE0..0xFF, Loader::ifnbitjmpref)
                    .set(0xFF, Loader::ifnbitjmpref),
            )
            .set(0xE4, Loader::repeat)
            .set(0xE5, Loader::repeatend)
            .set(0xE6, Loader::until)
            .set(0xE7, Loader::untilend)
            .set(0xE8, Loader::while_)
            .set(0xE9, Loader::whileend)
            .set(0xEA, Loader::again)
            .set(0xEB, Loader::againend)
            .set(0xEC, Loader::setcontargs)
            .add_subset(
                0xED,
                Handlers::new()
                    .set_range(0x00..0x10, Loader::returnargs)
                    .set(0x10, Loader::returnva)
                    .set(0x11, Loader::setcontva)
                    .set(0x12, Loader::setnumva)
                    .set(0x1E, Loader::bless)
                    .set(0x1F, Loader::blessva)
                    .set_range(0x40..0x50, Loader::pushctr)
                    .set_range(0x50..0x60, Loader::popctr)
                    .set_range(0x60..0x70, Loader::setcontctr)
                    .set_range(0x70..0x76, Loader::setretctr)
                    .set(0x77, Loader::setretctr)
                    .set_range(0x80..0x90, Loader::setaltctr)
                    .set_range(0x90..0xA0, Loader::popsave)
                    .set_range(0xA0..0xB0, Loader::save)
                    .set_range(0xB0..0xC0, Loader::savealt)
                    .set_range(0xC0..0xD0, Loader::saveboth)
                    .set(0xE0, Loader::pushctrx)
                    .set(0xE1, Loader::popctrx)
                    .set(0xE2, Loader::setcontctrx)
                    .set(0xE3, Loader::setcontctrmany)
                    .set(0xE4, Loader::setcontctrmanyx)
                    .set(0xF0, Loader::compos)
                    .set(0xF1, Loader::composalt)
                    .set(0xF2, Loader::composboth)
                    .set(0xF3, Loader::atexit)
                    .set(0xF4, Loader::atexitalt)
                    .set(0xF5, Loader::setexitalt)
                    .set(0xF6, Loader::thenret)
                    .set(0xF7, Loader::thenretalt)
                    .set(0xF8, Loader::invert)
                    .set(0xF9, Loader::booleval)
                    .set(0xFA, Loader::samealt)
                    .set(0xFB, Loader::samealt_save),
            )
            .set(0xEE, Loader::blessargs)
            .set(0xF0, Loader::call_short)
            .add_subset(
                0xF1,
                Handlers::new()
                    .set_range(0x00..0x40, Loader::call_long)
                    .set_range(0x40..0x80, Loader::jmp)
                    .set_range(0x80..0xC0, Loader::prepare),
            )
    }

    fn add_code_page_0_exceptions(&mut self) -> &mut Handlers {
        self.add_subset(
            0xF2,
            Handlers::new()
                .set_range(0x00..0x40, Loader::throw_short)
                .set_range(0x40..0x80, Loader::throwif_short)
                .set_range(0x80..0xC0, Loader::throwifnot_short)
                .set_range(0xC0..0xC8, Loader::throw_long)
                .set_range(0xC8..0xD0, Loader::throwarg)
                .set_range(0xD0..0xD8, Loader::throwif_long)
                .set_range(0xD8..0xE0, Loader::throwargif)
                .set_range(0xE0..0xE8, Loader::throwifnot_long)
                .set_range(0xE8..0xF0, Loader::throwargifnot)
                .set(0xF0, Loader::throwany)
                .set(0xF1, Loader::throwargany)
                .set(0xF2, Loader::throwanyif)
                .set(0xF3, Loader::throwarganyif)
                .set(0xF4, Loader::throwanyifnot)
                .set(0xF5, Loader::throwarganyifnot)
                .set(0xFF, Loader::try_),
        )
        .set(0xF3, Loader::tryargs)
    }

    fn add_code_page_0_blockchain(&mut self) -> &mut Handlers {
        self.add_subset(
            0xFA,
            Handlers::new()
                .set(0x00, Loader::ldgrams)
                .set(0x01, Loader::ldvarint16)
                .set(0x02, Loader::stgrams)
                .set(0x03, Loader::stvarint16)
                .set(0x04, Loader::ldvaruint32)
                .set(0x05, Loader::ldvarint32)
                .set(0x06, Loader::stvaruint32)
                .set(0x07, Loader::stvarint32)
                .set(0x40, Loader::ldmsgaddr::<Signaling>)
                .set(0x41, Loader::ldmsgaddr::<Quiet>)
                .set(0x42, Loader::parsemsgaddr::<Signaling>)
                .set(0x43, Loader::parsemsgaddr::<Quiet>)
                .set(0x44, Loader::rewrite_std_addr::<Signaling>)
                .set(0x45, Loader::rewrite_std_addr::<Quiet>)
                .set(0x46, Loader::rewrite_var_addr::<Signaling>)
                .set(0x47, Loader::rewrite_var_addr::<Quiet>)
                .set(0x48, Loader::ldstdaddr::<Signaling>)
                .set(0x49, Loader::ldstdaddr::<Quiet>)
                .set(0x50, Loader::ldoptstdaddr::<Signaling>)
                .set(0x51, Loader::ldoptstdaddr::<Quiet>)
                .set(0x52, Loader::ststdaddr::<Signaling>)
                .set(0x53, Loader::ststdaddr::<Quiet>)
                .set(0x54, Loader::stoptstdaddr::<Signaling>)
                .set(0x55, Loader::stoptstdaddr::<Quiet>),
        )
        .add_subset(
            0xFB,
            Handlers::new()
                .set(0x00, Loader::sendrawmsg)
                .set(0x02, Loader::rawreserve)
                .set(0x03, Loader::rawreservex)
                .set(0x04, Loader::setcode)
                .set(0x06, Loader::setlibcode)
                .set(0x07, Loader::changelib),
        )
    }

    fn add_code_page_0_dictionaries(&mut self) -> &mut Handlers {
        self.add_subset(
            0xF4,
            Handlers::new()
                .set(0x00, Loader::stdict)
                .set(0x01, Loader::skipdict)
                .set(0x02, Loader::lddicts)
                .set(0x03, Loader::plddicts)
                .set(0x04, Loader::lddict)
                .set(0x05, Loader::plddict)
                .set(0x06, Loader::lddictq)
                .set(0x07, Loader::plddictq)
                .set(0x0A, Loader::dictget)
                .set(0x0B, Loader::dictgetref)
                .set(0x0C, Loader::dictiget)
                .set(0x0D, Loader::dictigetref)
                .set(0x0E, Loader::dictuget)
                .set(0x0F, Loader::dictugetref)
                .set(0x12, Loader::dictset)
                .set(0x13, Loader::dictsetref)
                .set(0x14, Loader::dictiset)
                .set(0x15, Loader::dictisetref)
                .set(0x16, Loader::dictuset)
                .set(0x17, Loader::dictusetref)
                .set(0x1A, Loader::dictsetget)
                .set(0x1B, Loader::dictsetgetref)
                .set(0x1C, Loader::dictisetget)
                .set(0x1D, Loader::dictisetgetref)
                .set(0x1E, Loader::dictusetget)
                .set(0x1F, Loader::dictusetgetref)
                .set(0x22, Loader::dictreplace)
                .set(0x23, Loader::dictreplaceref)
                .set(0x24, Loader::dictireplace)
                .set(0x25, Loader::dictireplaceref)
                .set(0x26, Loader::dictureplace)
                .set(0x27, Loader::dictureplaceref)
                .set(0x2A, Loader::dictreplaceget)
                .set(0x2B, Loader::dictreplacegetref)
                .set(0x2C, Loader::dictireplaceget)
                .set(0x2D, Loader::dictireplacegetref)
                .set(0x2E, Loader::dictureplaceget)
                .set(0x2F, Loader::dictureplacegetref)
                .set(0x32, Loader::dictadd)
                .set(0x33, Loader::dictaddref)
                .set(0x34, Loader::dictiadd)
                .set(0x35, Loader::dictiaddref)
                .set(0x36, Loader::dictuadd)
                .set(0x37, Loader::dictuaddref)
                .set(0x3A, Loader::dictaddget)
                .set(0x3B, Loader::dictaddgetref)
                .set(0x3C, Loader::dictiaddget)
                .set(0x3D, Loader::dictiaddgetref)
                .set(0x3E, Loader::dictuaddget)
                .set(0x3F, Loader::dictuaddgetref)
                .set(0x41, Loader::dictsetb)
                .set(0x42, Loader::dictisetb)
                .set(0x43, Loader::dictusetb)
                .set(0x45, Loader::dictsetgetb)
                .set(0x46, Loader::dictisetgetb)
                .set(0x47, Loader::dictusetgetb)
                .set(0x49, Loader::dictreplaceb)
                .set(0x4A, Loader::dictireplaceb)
                .set(0x4B, Loader::dictureplaceb)
                .set(0x4D, Loader::dictreplacegetb)
                .set(0x4E, Loader::dictireplacegetb)
                .set(0x4F, Loader::dictureplacegetb)
                .set(0x51, Loader::dictaddb)
                .set(0x52, Loader::dictiaddb)
                .set(0x53, Loader::dictuaddb)
                .set(0x55, Loader::dictaddgetb)
                .set(0x56, Loader::dictiaddgetb)
                .set(0x57, Loader::dictuaddgetb)
                .set(0x59, Loader::dictdel)
                .set(0x5A, Loader::dictidel)
                .set(0x5B, Loader::dictudel)
                .set(0x62, Loader::dictdelget)
                .set(0x63, Loader::dictdelgetref)
                .set(0x64, Loader::dictidelget)
                .set(0x65, Loader::dictidelgetref)
                .set(0x66, Loader::dictudelget)
                .set(0x67, Loader::dictudelgetref)
                .set(0x69, Loader::dictgetoptref)
                .set(0x6A, Loader::dictigetoptref)
                .set(0x6B, Loader::dictugetoptref)
                .set(0x6D, Loader::dictsetgetoptref)
                .set(0x6E, Loader::dictisetgetoptref)
                .set(0x6F, Loader::dictusetgetoptref)
                .set(0x70, Loader::pfxdictset)
                .set(0x71, Loader::pfxdictreplace)
                .set(0x72, Loader::pfxdictadd)
                .set(0x73, Loader::pfxdictdel)
                .set(0x74, Loader::dictgetnext)
                .set(0x75, Loader::dictgetnexteq)
                .set(0x76, Loader::dictgetprev)
                .set(0x77, Loader::dictgetpreveq)
                .set(0x78, Loader::dictigetnext)
                .set(0x79, Loader::dictigetnexteq)
                .set(0x7A, Loader::dictigetprev)
                .set(0x7B, Loader::dictigetpreveq)
                .set(0x7C, Loader::dictugetnext)
                .set(0x7D, Loader::dictugetnexteq)
                .set(0x7E, Loader::dictugetprev)
                .set(0x7F, Loader::dictugetpreveq)
                .set(0x82, Loader::dictmin)
                .set(0x83, Loader::dictminref)
                .set(0x84, Loader::dictimin)
                .set(0x85, Loader::dictiminref)
                .set(0x86, Loader::dictumin)
                .set(0x87, Loader::dictuminref)
                .set(0x8A, Loader::dictmax)
                .set(0x8B, Loader::dictmaxref)
                .set(0x8C, Loader::dictimax)
                .set(0x8D, Loader::dictimaxref)
                .set(0x8E, Loader::dictumax)
                .set(0x8F, Loader::dictumaxref)
                .set(0x92, Loader::dictremmin)
                .set(0x93, Loader::dictremminref)
                .set(0x94, Loader::dictiremmin)
                .set(0x95, Loader::dictiremminref)
                .set(0x96, Loader::dicturemmin)
                .set(0x97, Loader::dicturemminref)
                .set(0x9A, Loader::dictremmax)
                .set(0x9B, Loader::dictremmaxref)
                .set(0x9C, Loader::dictiremmax)
                .set(0x9D, Loader::dictiremmaxref)
                .set(0x9E, Loader::dicturemmax)
                .set(0x9F, Loader::dicturemmaxref)
                .set(0xA0, Loader::dictigetjmp)
                .set(0xA1, Loader::dictugetjmp)
                .set(0xA2, Loader::dictigetexec)
                .set(0xA3, Loader::dictugetexec)
                .set_range(0xA4..0xA8, Loader::dictpushconst)
                .set(0xA8, Loader::pfxdictgetq)
                .set(0xA9, Loader::pfxdictget)
                .set(0xAA, Loader::pfxdictgetjmp)
                .set(0xAB, Loader::pfxdictgetexec)
                .set_range(0xAC..0xAF, Loader::pfxdictswitch)
                .set(0xAF, Loader::pfxdictswitch)
                .set(0xB1, Loader::subdictget)
                .set(0xB2, Loader::subdictiget)
                .set(0xB3, Loader::subdictuget)
                .set(0xB5, Loader::subdictrpget)
                .set(0xB6, Loader::subdictirpget)
                .set(0xB7, Loader::subdicturpget)
                .set(0xBC, Loader::dictigetjmpz)
                .set(0xBD, Loader::dictugetjmpz)
                .set(0xBE, Loader::dictigetexecz)
                .set(0xBF, Loader::dictugetexecz),
        )
    }

    fn add_code_page_0_gas_rand_config(&mut self) -> &mut Handlers {
        self.add_subset(
            0xF8,
            Handlers::new()
                .set(0x00, Loader::accept)
                .set(0x01, Loader::setgaslimit)
                .set(0x02, Loader::buygas)
                .set(0x04, Loader::gramtogas)
                .set(0x05, Loader::gastogram)
                .set(0x07, Loader::gasconsumed)
                .set(0x0F, Loader::commit)
                .set(0x10, Loader::randu256)
                .set(0x11, Loader::rand)
                .set(0x14, Loader::setrand)
                .set(0x15, Loader::addrand)
                .set(0x20, Loader::getparam)
                .set(0x21, Loader::getparam)
                .set(0x22, Loader::getparam)
                .set(0x23, Loader::now)
                .set(0x24, Loader::blocklt)
                .set(0x25, Loader::ltime)
                .set(0x26, Loader::randseed)
                .set(0x27, Loader::balance)
                .set(0x28, Loader::my_addr)
                .set(0x29, Loader::config_root)
                .set(0x2a, Loader::my_code)
                .set(0x30, Loader::config_dict)
                .set(0x32, Loader::config_ref_param)
                .set(0x33, Loader::config_opt_param)
                .set(0x40, Loader::getglobvar)
                .set_range(0x41..0x5F, Loader::getglob)
                .set(0x5F, Loader::getglob)
                .set(0x60, Loader::setglobvar)
                .set_range(0x61..0x7F, Loader::setglob)
                .set(0x7F, Loader::setglob),
        )
    }

    fn add_code_page_0_crypto(&mut self) -> &mut Handlers {
        self.add_subset(
            0xF9,
            Handlers::new()
                .set(0x00, Loader::hashcu)
                .set(0x01, Loader::hashsu)
                .set(0x02, Loader::sha256u)
                .set(0x10, Loader::chksignu)
                .set(0x11, Loader::chksigns)
                .set(0x16, Loader::hashbu)
                .set(0x40, Loader::cdatasizeq)
                .set(0x41, Loader::cdatasize)
                .set(0x42, Loader::sdatasizeq)
                .set(0x43, Loader::sdatasize),
        )
    }

    fn add_code_page_0_debug(&mut self) -> &mut Handlers {
        self.add_subset(
            0xFE,
            Handlers::new()
                .set(0x00, Loader::dump_stack)
                .set_range(0x01..0x0F, Loader::dump_stack_top)
                .set(0x10, Loader::dump_hex)
                .set(0x11, Loader::print_hex)
                .set(0x12, Loader::dump_bin)
                .set(0x13, Loader::print_bin)
                .set(0x14, Loader::dump_str)
                .set(0x15, Loader::print_str)
                .set(0x1E, Loader::debug_off)
                .set(0x1F, Loader::debug_on)
                .set_range(0x20..0x2F, Loader::dump_var)
                .set_range(0x30..0x3F, Loader::print_var)
                .set_range(0xF0..0xFF, Loader::dump_string)
                .set(0xFF, Loader::dump_string),
        )
    }

    pub(crate) fn get_handler(&self, slice: &mut SliceData) -> Result<LoadHandler> {
        let cmd = slice.get_next_byte()?;
        match self.directs[cmd as usize] {
            Handler::Direct(handler) => Ok(handler),
            Handler::Subset(i) => self.subsets[i].get_handler(slice),
        }
    }

    fn add_subset(&mut self, code: u8, subset: &mut Handlers) -> &mut Handlers {
        match self.directs[code as usize] {
            Handler::Direct(x) => {
                if x as usize == Loader::unknown as usize {
                    self.directs[code as usize] = Handler::Subset(self.subsets.len());
                    self.subsets.push(std::mem::replace(subset, Handlers::new()))
                } else {
                    panic!("Slot for subset {:02x} is already occupied", code)
                }
            }
            _ => panic!("Subset {:02x} is already registered", code),
        }
        self
    }

    fn register_handler(&mut self, code: u8, handler: LoadHandler) {
        match self.directs[code as usize] {
            Handler::Direct(x) => {
                if x as usize == Loader::unknown as usize {
                    self.directs[code as usize] = Handler::Direct(handler)
                } else {
                    panic!("Code {:02x} is already registered", code)
                }
            }
            _ => panic!("Slot for code {:02x} is already occupied", code),
        }
    }

    fn set(&mut self, code: u8, handler: LoadHandler) -> &mut Handlers {
        self.register_handler(code, handler);
        self
    }

    fn set_range(&mut self, codes: std::ops::Range<u8>, handler: LoadHandler) -> &mut Handlers {
        for code in codes {
            self.register_handler(code, handler);
        }
        self
    }
}
