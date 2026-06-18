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
use ton_assembler::CompileError;
use ton_block::{
    BuilderData, CellType, ExceptionCode, IBitstring, SimpleLib, SliceData, StateInitLib,
};
use ton_vm::{
    boolean, int,
    stack::{integer::IntegerData, Stack, StackItem},
};

mod common;
use common::*;

fn to_cell<T>(data: T) -> StackItem
where
    T: AsRef<[u8]>,
{
    create::cell(data)
}

mod ctos {
    use super::*;

    #[test]
    fn convert_cell_to_slice() {
        test_case(
            "NEWC
             ENDC
             CTOS",
        )
        .expect_item(StackItem::Slice(SliceData::new_empty()));
    }

    #[test]
    fn convert_cell_with_data_to_slice() {
        test_case(
            "PUSHINT 1
             NEWC
             STU 8
             ENDC
             CTOS",
        )
        .expect_item(StackItem::Slice(SliceData::new(vec![1, 0x80])));
    }

    #[test]
    fn convert_cell_library_to_slice() {
        let mut lib = BuilderData::default();
        lib.append_i8(3).unwrap();
        let lib = lib.into_cell().unwrap();
        let hash = lib.repr_hash().clone();

        let mut b = BuilderData::default();
        b.set_type(CellType::LibraryReference);
        b.append_i8(2).unwrap();
        b.append_bytestring(&hash.clone().into()).unwrap();

        let mut library = StateInitLib::default();
        library.set(&hash, &SimpleLib::new(lib, false)).unwrap();
        let library = library.inner();

        let cell = b.into_cell().unwrap();

        test_case_with_ref(
            "PUSHREF
             DUP
             DUP

             TEN
             CDATASIZEQ
             DROP2
             DROP2

             CTOS
             SWAP
             CTOS",
            cell,
        )
        .with_library(library)
        .expect_stack_extended(
            Stack::new()
                .push(StackItem::Slice(SliceData::new(vec![3, 0x80])))
                .push(StackItem::Slice(SliceData::new(vec![3, 0x80]))),
            None,
        );
    }
}

mod ldi {
    use super::*;

    #[test]
    //Get int from slice. Return pushed int
    fn get_unsign_number_from_slice() {
        test_case(
            "PUSHINT 1
             NEWC
             STU 8
             ENDC
             CTOS
             LDI 8",
        )
        .expect_stack(Stack::new().push(int!(1)).push(create::slice([0x80])));
    }

    #[test]
    //Get 0 bit from slice. Compilation error, because arg 0 from slice must be positive
    fn try_to_get_0bit_from_slice() {
        test_case(
            "PUSHINT 1
	     NEWC
	     STU 8
	     ENDC
	     CTOS
	     LDI 0",
        )
        .expect_compilation_failure(CompileError::out_of_range(6, 7, "LDI", "arg 0"));
    }

    #[test]
    //Get 7 and 9 bits from slice. Return 7 bit in first number and 9 bit in second (1) and third (8) numbers.
    fn try_to_get_7_9_from_slice() {
        test_case(
            "PUSHINT 1
	     PUSHINT 2
	     NEWC
	     STU 16
	     ENDC
	     CTOS
	     LDI 7
	     LDI 9",
        )
        .expect_stack(
            Stack::new().push(int!(1)).push(int!(0)).push(int!(2)).push(create::slice([0x80])),
        );
    }

    #[test]
    //Get 257 bit from slice. Compilation error, because arg 0 from slice must be less than 255 + 1
    fn try_to_get_257bit_from_slice() {
        test_case(
            "PUSHINT 1
	     NEWC
	     STU 8
	     ENDC
	     CTOS
	     LDI 257",
        )
        .expect_compilation_failure(CompileError::out_of_range(6, 7, "LDI", "arg 0"));
    }

    #[test]
    //Get max permitted bits from slice. Return pushed int
    fn get_unsign_number_from_slice_with_max() {
        test_case(
            "PUSHINT 1
             NEWC
             STU 256
             ENDC
             CTOS
             LDI 256",
        )
        .expect_stack(Stack::new().push(int!(1)).push(create::slice([0x80])));
    }

    #[test]
    //Try to get more bits than are located in slice. Error - Cell Underflow
    fn try_to_get_unsign_number_from_too_short_slice() {
        test_case(
            "PUSHINT 1
             NEWC
             STU 8
             ENDC
             CTOS
             LDI 100",
        )
        .expect_failure(ExceptionCode::CellUnderflow);
    }

    #[test]
    //Try to get number from emptiness. Error - Stack Underflow
    fn get_unsign_number_from_slice_err_no_slice() {
        test_case("LDI 8").expect_failure(ExceptionCode::StackUnderflow);
    }
}

mod ldu {
    use super::*;

    #[test]
    fn get_unsign_number_from_slice() {
        test_case(
            "PUSHINT 1
             NEWC
             STU 8
             ENDC
             CTOS
             LDU 8",
        )
        .expect_stack(Stack::new().push(int!(1)).push(create::slice([0x80])));
    }

    #[test]
    fn get_unsign_number_from_slice_err_no_number() {
        test_case(
            "NEWC
             ENDC
             CTOS
             LDU 8",
        )
        .expect_failure(ExceptionCode::CellUnderflow);
    }

    #[test]
    fn get_unsign_number_from_slice_err_no_slice() {
        test_case("LDU 8").expect_failure(ExceptionCode::StackUnderflow);
    }
}

mod lduq {
    use super::*;

    #[test]
    fn get_unsign_number_from_slice() {
        test_case(
            "PUSHINT 1
             NEWC
             STU 8
             ENDC
             CTOS
             LDUQ 8",
        )
        .expect_stack(Stack::new().push(int!(1)).push(create::slice([0x80])).push(boolean!(true)));
    }

    #[test]
    fn try_to_get_unsign_number_from_too_short_slice() {
        test_case(
            "PUSHINT 1
             NEWC
             STU 8
             ENDC
             CTOS
             LDUQ 100",
        )
        .expect_stack(Stack::new().push(create::slice([0x01, 0x80])).push(boolean!(false)));
    }

    #[test]
    fn get_unsign_number_from_slice_err_no_slice() {
        test_case("LDUQ 8").expect_failure(ExceptionCode::StackUnderflow);
    }
}

mod ldiq {
    use super::*;

    #[test]
    fn get_unsign_number_from_slice() {
        test_case(
            "PUSHINT 1
             NEWC
             STU 8
             ENDC
             CTOS
             LDIQ 8",
        )
        .expect_stack(Stack::new().push(int!(1)).push(create::slice([0x80])).push(boolean!(true)));
    }

    #[test]
    fn try_to_get_unsign_number_from_too_short_slice() {
        test_case(
            "PUSHINT 1
             NEWC
             STU 8
             ENDC
             CTOS
             LDIQ 100",
        )
        .expect_stack(Stack::new().push(create::slice([0x01, 0x80])).push(boolean!(false)));
    }

    #[test]
    fn get_unsign_number_from_slice_err_no_slice() {
        test_case("LDIQ 8").expect_failure(ExceptionCode::StackUnderflow);
    }
}

mod lduxq {
    use super::*;

    #[test]
    fn get_unsign_number_from_slice() {
        test_case(
            "PUSHINT 1
             NEWC
             STU 8
             ENDC
             CTOS
             PUSHINT 8
             LDUXQ",
        )
        .expect_stack(Stack::new().push(int!(1)).push(create::slice([0x80])).push(boolean!(true)));
    }

    #[test]
    fn try_to_get_unsign_number_from_too_short_slice() {
        test_case(
            "PUSHINT 1
             NEWC
             STU 8
             ENDC
             CTOS
             PUSHINT 100
             LDUXQ",
        )
        .expect_stack(Stack::new().push(create::slice([0x01, 0x80])).push(boolean!(false)));
    }

    #[test]
    fn get_unsign_number_from_slice_err_empty_stack() {
        test_case("LDUXQ").expect_failure(ExceptionCode::StackUnderflow);
    }
}

mod ldix {
    use super::*;

    #[test]
    fn get_unsign_number_from_slice() {
        test_case(
            "PUSHINT 1
             NEWC
             STU 8
             ENDC
             CTOS
             PUSHINT 8
             LDIX",
        )
        .expect_stack(Stack::new().push(int!(1)).push(create::slice([0x80])));
    }

    #[test]
    fn try_to_get_unsign_number_from_too_short_slice() {
        test_case(
            "PUSHINT 1
             NEWC
             STU 8
             ENDC
             CTOS
             PUSHINT 100
             LDIX",
        )
        .expect_failure(ExceptionCode::CellUnderflow);
    }

    #[test]
    fn get_unsign_number_from_slice_err_empty_stack() {
        test_case("LDIX").expect_failure(ExceptionCode::StackUnderflow);
    }

    #[test]
    fn get_unsign_number_from_slice_err_range_check_eror() {
        test_case(
            "
            NEWC
            ZERO
            STIR 256
            STSLICECONST x0
            ENDC
            CTOS
            PUSHINT 258
            LDIX
        ",
        )
        .expect_failure(ExceptionCode::RangeCheckError);
    }

    #[test]
    fn get_unsign_number_from_slice_max_length_normal() {
        test_case(
            "
            NEWC
            ZERO
            PUSHINT 257
            STIXR
            ENDC
            CTOS
            PUSHINT 257
            LDIX
            ENDS
        ",
        )
        .expect_item(int!(0));
    }
}

mod ldixq {
    use super::*;

    #[test]
    fn get_unsign_number_from_slice() {
        test_case(
            "PUSHINT 1
             NEWC
             STU 8
             ENDC
             CTOS
             PUSHINT 8
             LDIXQ",
        )
        .expect_stack(Stack::new().push(int!(1)).push(create::slice([0x80])).push(boolean!(true)));
    }

    #[test]
    fn try_to_get_unsign_number_from_too_short_slice() {
        test_case(
            "PUSHINT 1
             NEWC
             STU 8
             ENDC
             CTOS
             PUSHINT 100
             LDIXQ",
        )
        .expect_stack(Stack::new().push(create::slice([0x01, 0x80])).push(boolean!(false)));
    }

    #[test]
    fn get_unsign_number_from_slice_err_empty_stack() {
        test_case("LDIXQ").expect_failure(ExceptionCode::StackUnderflow);
    }
}

mod ldref {
    use super::*;

    #[test]
    fn get_reference_from_slice() {
        test_case(
            "NEWC
             ENDC
             NEWC
             STREF
             ENDC
             CTOS
             LDREF",
        )
        .expect_stack(
            Stack::new().push(create::cell([0x80])).push(StackItem::Slice(SliceData::new_empty())),
        );
    }

    #[test]
    fn get_reference_from_slice_run_quiet() {
        test_case(
            "NEWC
             ENDC
             NEWC
             STREFQ
             PUSHCONT {
                ENDC
                CTOS
                LDREF
             }
             IFNOT",
        )
        .expect_stack(
            Stack::new().push(to_cell([0x80])).push(StackItem::Slice(SliceData::new_empty())),
        );
    }

    #[test]
    fn get_reference_from_slice_run_rev_quiet() {
        test_case(
            "NEWC
             ENDC
             NEWC
             SWAP
             STREFRQ
             PUSHCONT {
                ENDC
                CTOS
                LDREF
             }
             IFNOT",
        )
        .expect_stack(
            Stack::new().push(to_cell([0x80])).push(StackItem::Slice(SliceData::new_empty())),
        );
    }

    #[test]
    fn get_reference_with_data_from_slice() {
        test_case(
            "PUSHINT 1
             NEWC
             STU 8
             ENDC
             NEWC
             STREF
             ENDC
             CTOS
             LDREF",
        )
        .expect_stack(
            Stack::new()
                .push(create::cell([1, 0x80]))
                .push(StackItem::Slice(SliceData::new_empty())),
        );
    }

    #[test]
    fn get_reference_from_slice_err_no_ref() {
        test_case(
            "NEWC
             ENDC
             CTOS
             LDREF",
        )
        .expect_failure(ExceptionCode::CellUnderflow);
    }

    #[test]
    fn get_reference_from_slice_err_no_slice() {
        test_case("LDREF").expect_failure(ExceptionCode::StackUnderflow);
    }
}

mod ldreftos {
    use super::*;

    #[test]
    fn basic_scenario() {
        test_case(
            "PUSHINT 1
             NEWC
             STU 8
             ENDC
             NEWC
             STREF
             ENDC
             CTOS
             LDREFRTOS",
        )
        .expect_stack(
            Stack::new()
                .push(StackItem::Slice(SliceData::new_empty()))
                .push(create::slice([0x01, 0x80])),
        );
    }

    #[test]
    fn convert_cell_library_to_slice() {
        let mut lib = BuilderData::default();
        lib.append_i8(3).unwrap();
        let lib = lib.into_cell().unwrap();
        let hash = lib.repr_hash().clone();

        let mut b = BuilderData::default();
        b.set_type(CellType::LibraryReference);
        b.append_i8(2).unwrap();
        b.append_bytestring(&hash.clone().into()).unwrap();

        let mut library = StateInitLib::default();
        library.set(&hash, &SimpleLib::new(lib, false)).unwrap();
        let library = library.inner();

        let mut wrapper = BuilderData::new();
        wrapper.append_i8(5).unwrap();
        wrapper.checked_append_reference(b.into_cell().unwrap()).unwrap();

        let cell = wrapper.into_cell().unwrap();

        test_case_with_ref(
            "PUSHREF
             CTOS
             LDREFRTOS
             SWAP
             DROP",
            cell,
        )
        .with_library(library)
        .expect_item(StackItem::Slice(SliceData::new(vec![3, 0x80])));
    }
}

mod pldux {
    use super::*;

    #[test]
    fn preload_unsign_len_number_from_slice() {
        test_case(
            "PUSHINT 1
             NEWC
             STU 8
             ENDC
             CTOS
             PUSHINT 8
             PLDUX",
        )
        .expect_item(int!(1));
    }

    #[test]
    fn preload_unsign_len_number_from_slice_err_no_number() {
        test_case(
            "NEWC
             ENDC
             CTOS
             PUSHINT 8
             PLDUX",
        )
        .expect_failure(ExceptionCode::CellUnderflow);
    }

    #[test]
    fn preload_unsign_len_number_from_slice_err_no_len() {
        test_case(
            "PUSHINT 2
             PUSHINT 1
             NEWC
             STU 8
             ENDC
             CTOS
             PLDUX",
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }

    #[test]
    fn preload_unsign_len_number_from_slice_err_no_slice() {
        test_case(
            "PUSHINT 8
             PLDUX",
        )
        .expect_failure(ExceptionCode::StackUnderflow);
    }
}

mod plduq {
    use super::*;

    #[test]
    fn preload_unsign_len_number_from_slice() {
        test_case(
            "PUSHINT 1
             NEWC
             STU 8
             ENDC
             CTOS
             PLDUQ 8",
        )
        .expect_stack(Stack::new().push(int!(1)).push(boolean!(true)));
    }

    #[test]
    fn preload_unsign_len_number_from_slice_empty_slice() {
        test_case(
            "NEWC
             ENDC
             CTOS
             PLDUQ 8",
        )
        .expect_stack(Stack::new().push(boolean!(false)));
    }

    #[test]
    fn preload_unsign_len_number_from_slice_err_no_slice() {
        test_case("PLDUQ 8").expect_failure(ExceptionCode::StackUnderflow);
    }
}

mod pldiq {
    use super::*;

    #[test]
    fn preload_unsign_len_number_from_slice() {
        test_case(
            "PUSHINT 1
             NEWC
             STU 8
             ENDC
             CTOS
             PLDIQ 8",
        )
        .expect_stack(Stack::new().push(int!(1)).push(boolean!(true)));
    }

    #[test]
    fn preload_unsign_len_number_from_slice_empty_slice() {
        test_case(
            "NEWC
             ENDC
             CTOS
             PLDIQ 8",
        )
        .expect_stack(Stack::new().push(boolean!(false)));
    }

    #[test]
    fn preload_unsign_len_number_from_slice_err_no_slice() {
        test_case("PLDIQ 8").expect_failure(ExceptionCode::StackUnderflow);
    }
}

mod plduxq {
    use super::*;

    #[test]
    fn preload_unsign_len_number_from_slice() {
        test_case(
            "PUSHINT 1
             NEWC
             STU 8
             ENDC
             CTOS
             PUSHINT 8
             PLDUXQ",
        )
        .expect_stack(Stack::new().push(int!(1)).push(boolean!(true)));
    }

    #[test]
    fn preload_unsign_len_number_from_slice_empty_slice() {
        test_case(
            "NEWC
             ENDC
             CTOS
             PUSHINT 8
             PLDUXQ",
        )
        .expect_stack(Stack::new().push(boolean!(false)));
    }

    #[test]
    fn preload_unsign_len_number_from_slice_err_no_len() {
        test_case(
            "PUSHINT 2
             PUSHINT 1
             NEWC
             STU 8
             ENDC
             CTOS
             PLDUXQ",
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }

    #[test]
    fn preload_unsign_len_number_from_slice_err_no_slice() {
        test_case(
            "PUSHINT 8
             PLDUXQ",
        )
        .expect_failure(ExceptionCode::StackUnderflow);
    }
}

mod pldix {
    use super::*;

    #[test]
    fn preload_sign_len_number_from_slice() {
        test_case(
            "PUSHINT 1
             NEWC
             STU 8
             ENDC
             CTOS
             PUSHINT 8
             PLDIX",
        )
        .expect_item(int!(1));
    }

    #[test]
    fn preload_sign_len_number_from_slice_empty_slice() {
        test_case(
            "NEWC
             ENDC
             CTOS
             PUSHINT 8
             PLDIX",
        )
        .expect_failure(ExceptionCode::CellUnderflow);
    }

    #[test]
    fn preload_sign_len_number_from_slice_err_no_len() {
        test_case(
            "PUSHINT 2
             PUSHINT 1
             NEWC
             STU 8
             ENDC
             CTOS
             PLDIX",
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }

    #[test]
    fn preload_sign_len_number_from_slice_err_no_slice() {
        test_case(
            "PUSHINT 8
             PLDIX",
        )
        .expect_failure(ExceptionCode::StackUnderflow);
    }
}

mod pldixq {
    use super::*;

    #[test]
    fn preload_unsign_len_number_from_slice() {
        test_case(
            "PUSHINT 1
             NEWC
             STU 8
             ENDC
             CTOS
             PUSHINT 8
             PLDIXQ",
        )
        .expect_stack(Stack::new().push(int!(1)).push(boolean!(true)));
    }

    #[test]
    fn preload_unsign_len_number_from_slice_empty_slice() {
        test_case(
            "NEWC
             ENDC
             CTOS
             PUSHINT 8
             PLDIXQ",
        )
        .expect_stack(Stack::new().push(boolean!(false)));
    }

    #[test]
    fn preload_unsign_len_number_from_slice_err_no_len() {
        test_case(
            "PUSHINT 2
             PUSHINT 1
             NEWC
             STU 8
             ENDC
             CTOS
             PLDIXQ",
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }

    #[test]
    fn preload_unsign_len_number_from_slice_err_no_slice() {
        test_case(
            "PUSHINT 8
             PLDIXQ",
        )
        .expect_failure(ExceptionCode::StackUnderflow);
    }
}

mod ldux {
    use super::*;

    #[test]
    fn get_unsign_len_number_from_slice() {
        test_case(
            "PUSHINT 1
             NEWC
             STU 8
             ENDC
             CTOS
             ZERO
             LDUX
             PUSHINT 8
             LDUX",
        )
        .expect_stack(
            Stack::new()
                .push(int!(0))
                .push(int!(1))
                .push(StackItem::Slice(SliceData::new(vec![0x80]))),
        );
    }

    #[test]
    fn get_unsign_len_number_from_slice_err_no_number() {
        test_case(
            "NEWC
             ENDC
             CTOS
             PUSHINT 8
             LDUX",
        )
        .expect_failure(ExceptionCode::CellUnderflow);
    }

    #[test]
    fn get_unsign_len_number_from_slice_err_no_len() {
        test_case(
            "PUSHINT 2
             PUSHINT 1
             NEWC
             STU 8
             ENDC
             CTOS
             LDUX",
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }

    #[test]
    fn get_unsign_len_number_from_slice_err_no_slice() {
        test_case(
            "
            PUSHINT 8
            LDUX
        ",
        )
        .expect_failure(ExceptionCode::StackUnderflow);
    }
}

mod plduz {
    use super::*;

    #[test]
    fn basic_scenario() {
        test_case(
            "
            PUSHSLICE x0102030480_
            PLDUZ 32
        ",
        )
        .expect_stack(
            Stack::new()
                .push(StackItem::Slice(SliceData::new(vec![0x01, 0x02, 0x03, 0x04, 0x80])))
                .push(int!(0x01020304)),
        );
    }

    #[test]
    fn small_slice() {
        test_case(
            "
            PUSHSLICE x02030480_
            PLDUZ 32
        ",
        )
        .expect_stack(
            Stack::new()
                .push(StackItem::Slice(SliceData::new(vec![0x02, 0x03, 0x04, 0x80])))
                .push(int!(0x02030400)),
        );
    }

    #[test]
    fn basic_scenario_64() {
        test_case(
            "
            PUSHSLICE x01020304050607080980_
            PLDUZ 64
        ",
        )
        .expect_stack(
            Stack::new()
                .push(StackItem::Slice(SliceData::new(vec![
                    0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x80,
                ])))
                .push(int!(0x0102030405060708u64)),
        );
    }
}

mod ldslicex {
    use super::*;

    #[test]
    fn get_slice_len_from_slice() {
        test_case(
            "PUSHSLICE xCD8_
             NEWC
             STSLICE
             ENDC
             CTOS
             ZERO
             LDSLICEX
             PUSHINT 5
             LDSLICEX
             ZERO
             LDSLICEX
             PUSHINT 3
             LDSLICEX
             ZERO
             LDSLICEX",
        )
        .expect_stack(
            Stack::new()
                .push(StackItem::Slice(SliceData::default()))
                .push(StackItem::Slice(SliceData::new(vec![0xCC])))
                .push(StackItem::Slice(SliceData::default()))
                .push(StackItem::Slice(SliceData::new(vec![0xB0])))
                .push(StackItem::Slice(SliceData::default()))
                .push(StackItem::Slice(SliceData::new(vec![0x80]))),
        );
    }

    #[test]
    fn get_slice_len_from_slice_err_no_slice() {
        test_case(
            "NEWC
             ENDC
             CTOS
             PUSHINT 8
             LDSLICEX",
        );
    } // TODO: check test validity

    #[test]
    fn get_slice_len_from_sliceet_slice_len_from_slice_err_no_len() {
        test_case(
            "PUSHSLICE xC_
             NEWC
             STSLICE
             ENDC
             CTOS
             LDSLICEX",
        );
    } // TODO: check test validity

    #[test]
    fn get_slice_len_from_slice_err_no_storing_slice() {
        test_case(
            "PUSHINT 8
             LDSLICEX",
        )
        .expect_failure(ExceptionCode::StackUnderflow);
    }
}

mod ldslicexq {
    use super::*;

    #[test]
    fn get_slice_len_from_slice() {
        test_case(
            "PUSHSLICE xC08_
             NEWC
             STSLICE
             ENDC
             CTOS
             PUSHINT 8
             LDSLICEXQ",
        )
        .expect_stack(
            Stack::new()
                .push(StackItem::Slice(SliceData::new(vec![0xC0, 0x80])))
                .push(StackItem::Slice(SliceData::new(vec![0x80])))
                .push(boolean!(true)),
        );
    }

    #[test]
    fn get_slice_len_from_empty_slice() {
        test_case(
            "NEWC
             ENDC
             CTOS
             PUSHINT 8
             LDSLICEXQ",
        )
        .expect_stack(
            Stack::new().push(StackItem::Slice(SliceData::new(vec![0x80]))).push(boolean!(false)),
        );
    }

    #[test]
    fn get_slice_len_from_sliceet_slice_len_from_slice_err_no_len() {
        test_case(
            "PUSHSLICE xC_
             NEWC
             STSLICE
             ENDC
             CTOS
             LDSLICEXQ",
        )
        .expect_failure(ExceptionCode::StackUnderflow);
    }

    #[test]
    fn get_slice_len_from_slice_err_no_storing_slice() {
        test_case(
            "PUSHINT 8
             LDSLICEXQ",
        )
        .expect_failure(ExceptionCode::StackUnderflow);
    }
}

mod pldslicexq {
    use super::*;

    #[test]
    fn basic_success() {
        test_case(
            "
            PUSHSLICE x779980_
            PUSHINT 8
            PLDSLICEXQ
        ",
        )
        .expect_stack(
            Stack::new()
                .push(StackItem::Slice(SliceData::new(vec![0x77, 0x80])))
                .push(boolean!(true)),
        );
    }

    #[test]
    fn basic_failure() {
        test_case(
            "
            PUSHSLICE x8_
            PUSHINT 8
            PLDSLICEXQ
        ",
        )
        .expect_stack(Stack::new().push(boolean!(false)));
    }
}

mod pldslicex {
    use super::*;

    #[test]
    fn preload_slice_len_from_slice() {
        test_case(
            "PUSHSLICE xC_
             NEWC
             STSLICE
             ENDC
             CTOS
             PUSHINT 1
             PLDSLICEX",
        )
        .expect_item(StackItem::Slice(SliceData::new(vec![0xC0])));
    }

    #[test]
    fn preload_slice_len_from_slice_err_no_slice() {
        test_case(
            "NEWC
             ENDC
             CTOS
             PUSHINT 8
             PLDSLICEX",
        );
    } // TODO: check test validity

    #[test]
    fn preload_slice_len_from_slice_err_no_len() {
        test_case(
            "PUSHSLICE xC_
             NEWC
             STSLICE
             ENDC
             CTOS
             PLDSLICEX",
        );
    } // TODO: check test validity

    #[test]
    fn preload_slice_len_from_slice_err_no_storing_slice() {
        test_case(
            "PUSHINT 8
             PLDSLICEX",
        )
        .expect_failure(ExceptionCode::StackUnderflow);
    }
}

mod ldslice {
    use super::*;

    #[test]
    fn get_slice_len_from_slice() {
        test_case(
            "PUSHSLICE xC08_
             NEWC
             STSLICE
             ENDC
             CTOS
             LDSLICE 8",
        )
        .expect_stack(
            Stack::new()
                .push(StackItem::Slice(SliceData::new(vec![0xC0, 0x80])))
                .push(StackItem::Slice(SliceData::new(vec![0x80]))),
        );
    }

    #[test]
    fn get_slice_len_from_slice_err_no_storing_slice() {
        test_case("LDSLICE 8").expect_failure(ExceptionCode::StackUnderflow);
    }
}

mod ldsliceq {
    use super::*;

    #[test]
    fn get_slice_len_from_slice() {
        test_case(
            "PUSHSLICE xC08_
             NEWC
             STSLICE
             ENDC
             CTOS
             LDSLICEQ 8",
        )
        .expect_stack(
            Stack::new()
                .push(StackItem::Slice(SliceData::new(vec![0xC0, 0x80])))
                .push(StackItem::Slice(SliceData::new(vec![0x80])))
                .push(boolean!(true)),
        );
    }

    #[test]
    fn get_slice_len_from_empty_slice() {
        test_case(
            "NEWC
             ENDC
             CTOS
             LDSLICEQ 8",
        )
        .expect_stack(
            Stack::new().push(StackItem::Slice(SliceData::new(vec![0x80]))).push(boolean!(false)),
        );
    }

    #[test]
    fn get_slice_len_from_slice_err_empty_stack() {
        test_case("LDSLICEQ 8").expect_failure(ExceptionCode::StackUnderflow);
    }
}

mod pldslice {
    use super::*;

    #[test]
    fn basic_scenario() {
        test_case(
            "
            PUSHSLICE x779980_
            PLDSLICE 8
        ",
        )
        .expect_stack(Stack::new().push(StackItem::Slice(SliceData::new(vec![0x77, 0x80]))));
    }

    #[test]
    fn basic_failure() {
        test_case(
            "
            PUSHSLICE x8_
            PLDSLICE 8
        ",
        )
        .expect_failure(ExceptionCode::CellUnderflow);
    }
}

mod pldsliceq {
    use super::*;

    #[test]
    fn basic_success() {
        test_case(
            "
            PUSHSLICE x779980_
            PLDSLICEQ 8
        ",
        )
        .expect_stack(
            Stack::new()
                .push(StackItem::Slice(SliceData::new(vec![0x77, 0x80])))
                .push(boolean!(true)),
        );
    }

    #[test]
    fn basic_failure() {
        test_case(
            "
            PUSHSLICE x8_
            PLDSLICEQ 8
        ",
        )
        .expect_stack(Stack::new().push(boolean!(false)));
    }
}

mod sdcutfirst {
    use super::*;

    #[test]
    fn basic_scenario() {
        test_case(
            "
            PUSHSLICE x779980_
            PUSHINT 8
            SDCUTFIRST
        ",
        )
        .expect_stack(Stack::new().push(StackItem::Slice(SliceData::new(vec![0x77, 0x80]))));
    }
}

mod sdskipfirst {
    use super::*;

    #[test]
    fn basic_scenario() {
        test_case(
            "
            PUSHSLICE x7766559980_
            PUSHINT 16
            SDSKIPFIRST
        ",
        )
        .expect_stack(Stack::new().push(StackItem::Slice(SliceData::new(vec![0x55, 0x99, 0x80]))));
    }
}

mod sdcutlast {
    use super::*;

    #[test]
    fn basic_scenario() {
        test_case(
            "
            PUSHSLICE x7766559980_
            PUSHINT 16
            SDCUTLAST
        ",
        )
        .expect_stack(Stack::new().push(StackItem::Slice(SliceData::new(vec![0x55, 0x99, 0x80]))));
    }
}

mod sdskiplast {
    use super::*;

    #[test]
    fn basic_scenario() {
        test_case(
            "
            PUSHSLICE x779980_
            PUSHINT 8
            SDSKIPLAST
        ",
        )
        .expect_stack(Stack::new().push(StackItem::Slice(SliceData::new(vec![0x77, 0x80]))));
    }
}

mod sdsubstr {
    use super::*;

    #[test]
    fn basic_scenario() {
        test_case(
            "
            PUSHSLICE x779980_
            PUSHINT 4
            PUSHINT 8
            SDSUBSTR
        ",
        )
        .expect_stack(Stack::new().push(StackItem::Slice(SliceData::new(vec![0x79, 0x80]))));
    }
}

mod sdbeginsx {
    use super::*;

    #[test]
    fn basic_scenarios() {
        test_case(
            "
            PUSHSLICE x11223344556677889980_
            PUSHSLICE x112233445566778880_
            SDBEGINSX
        ",
        )
        .expect_stack(Stack::new().push(StackItem::Slice(SliceData::new(vec![0x99, 0x80]))));
    }

    #[test]
    fn same_slices() {
        test_case(
            "
            PUSHSLICE x779980_
            PUSHSLICE x779980_
            SDBEGINSX
        ",
        )
        .expect_stack(Stack::new().push(StackItem::Slice(SliceData::new(vec![0x80]))));
    }

    #[test]
    fn no_prefix() {
        test_case(
            "
            PUSHSLICE x779980_
            PUSHSLICE x1180_
            SDBEGINSX
        ",
        )
        .expect_failure(ExceptionCode::CellUnderflow);
    }

    #[test]
    fn too_short_slice() {
        test_case(
            "
            PUSHSLICE x80_
            PUSHSLICE xFF80_
            SDBEGINSX
        ",
        )
        .expect_failure(ExceptionCode::CellUnderflow);
    }
}

mod ldule4q {
    use super::*;

    #[test]
    fn get_unsign_number_from_slice() {
        test_case(
            "PUSHSLICE xFFFFFFFF8_
             LDULE4Q",
        )
        .expect_stack(
            Stack::new().push(int!(4294967295u32)).push(create::slice([0x80])).push(boolean!(true)),
        );
    }

    #[test]
    fn try_to_get_unsign_number_from_empty_slice() {
        test_case(
            "NEWC
             ENDC
             CTOS
             LDULE4Q",
        )
        .expect_stack(Stack::new().push(create::slice([0x80])).push(boolean!(false)));
    }

    #[test]
    fn get_unsign_number_from_slice_err_empty_stack() {
        test_case("LDULE4Q").expect_failure(ExceptionCode::StackUnderflow);
    }
}

mod ldule8q {
    use super::*;

    #[test]
    fn get_unsign_number_from_slice() {
        test_case(
            "PUSHSLICE xFFFFFFFFFFFFFFFFFF8_
             LDULE8Q
             DROP
             SWAP
             DROP",
        )
        .expect_stack(Stack::new().push(create::slice([0xFF, 0x80])));
    }

    #[test]
    fn try_to_get_unsign_number_from_empty_slice() {
        test_case(
            "NEWC
             ENDC
             CTOS
             LDULE8Q",
        )
        .expect_stack(Stack::new().push(create::slice([0x80])).push(boolean!(false)));
    }

    #[test]
    fn get_unsign_number_from_slice_err_empty_stack() {
        test_case("LDULE8Q").expect_failure(ExceptionCode::StackUnderflow);
    }
}

mod pldule4q {
    use super::*;

    #[test]
    fn get_unsign_number_from_slice() {
        test_case(
            "PUSHSLICE xFFFFFFFF8_
             PLDULE4Q",
        )
        .expect_stack(Stack::new().push(int!(4294967295u32)).push(boolean!(true)));
    }

    #[test]
    fn try_to_get_unsign_number_from_empty_slice() {
        test_case(
            "NEWC
             ENDC
             CTOS
             PLDULE4Q",
        )
        .expect_stack(Stack::new().push(boolean!(false)));
    }

    #[test]
    fn get_unsign_number_from_slice_err_empty_stack() {
        test_case("PLDULE4Q").expect_failure(ExceptionCode::StackUnderflow);
    }
}

mod pldule8q {
    use super::*;

    #[test]
    fn get_unsign_number_from_slice() {
        test_case(
            "PUSHSLICE xFFFFFFFFFFFFFFFFFF0_
             PLDULE8Q
             SWAP
             DROP",
        )
        .expect_stack(Stack::new().push(boolean!(true)));
    }

    #[test]
    fn try_to_get_unsign_number_from_empty_slice() {
        test_case(
            "NEWC
             ENDC
             CTOS
             PLDULE8Q",
        )
        .expect_stack(Stack::new().push(boolean!(false)));
    }

    #[test]
    fn get_unsign_number_from_slice_err_empty_stack() {
        test_case("PLDULE8Q").expect_failure(ExceptionCode::StackUnderflow);
    }
}

mod ldule4 {
    use super::*;

    #[test]
    fn get_unsign_number_from_slice() {
        test_case(
            "PUSHSLICE xFFFFFFFF8_
             LDULE4",
        )
        .expect_stack(Stack::new().push(int!(4294967295u32)).push(create::slice([0x80])));
    }

    #[test]
    fn try_to_get_unsign_number_from_empty_slice() {
        test_case(
            "NEWC
             ENDC
             CTOS
             LDULE4",
        )
        .expect_failure(ExceptionCode::CellUnderflow);
    }

    #[test]
    fn get_unsign_number_from_slice_err_empty_stack() {
        test_case("LDULE4").expect_failure(ExceptionCode::StackUnderflow);
    }
}

mod ldule8 {
    use super::*;

    #[test]
    fn get_unsign_number_from_slice() {
        test_case(
            "PUSHSLICE xFFFFFFFFFFFFFFFFFF8_
             LDULE8
             SWAP
             DROP",
        )
        .expect_stack(Stack::new().push(create::slice([0xFF, 0x80])));
    }

    #[test]
    fn try_to_get_unsign_number_from_empty_slice() {
        test_case(
            "NEWC
             ENDC
             CTOS
             LDULE8",
        )
        .expect_failure(ExceptionCode::CellUnderflow);
    }

    #[test]
    fn get_unsign_number_from_slice_err_empty_stack() {
        test_case("LDULE8").expect_failure(ExceptionCode::StackUnderflow);
    }
}

mod pldule4 {
    use super::*;

    #[test]
    fn get_unsign_number_from_slice() {
        test_case(
            "PUSHSLICE xFFFFFFFF8_
             PLDULE4",
        )
        .expect_stack(Stack::new().push(int!(4294967295u32)));
    }

    #[test]
    fn try_to_get_unsign_number_from_empty_slice() {
        test_case(
            "NEWC
             ENDC
             CTOS
             PLDULE4",
        )
        .expect_failure(ExceptionCode::CellUnderflow);
    }

    #[test]
    fn get_unsign_number_from_slice_err_empty_stack() {
        test_case("PLDULE4").expect_failure(ExceptionCode::StackUnderflow);
    }
}

mod pldule8 {
    use super::*;

    #[test]
    fn try_to_get_unsign_number_from_empty_slice() {
        test_case(
            "NEWC
             ENDC
             CTOS
             PLDULE8",
        )
        .expect_failure(ExceptionCode::CellUnderflow);
    }

    #[test]
    fn get_unsign_number_from_slice_err_empty_stack() {
        test_case("PLDULE8").expect_failure(ExceptionCode::StackUnderflow);
    }
}

mod schkbits {
    use super::*;

    #[test]
    fn basic_success() {
        test_case(
            "
            PUSHSLICE x779980_
            PUSHINT 8
            SCHKBITS
        ",
        )
        .expect_stack(&Stack::new());
    }

    #[test]
    fn basic_failure() {
        test_case(
            "
            PUSHSLICE x48_
            PUSHINT 8
            SCHKBITS
        ",
        )
        .expect_failure(ExceptionCode::CellUnderflow);
    }
}

mod sbits {
    use super::*;

    #[test]
    fn basic_success() {
        test_case(
            "
            PUSHSLICE x779980_
            SBITS
        ",
        )
        .expect_item(int!(16));
    }
}

mod srefs {
    use super::*;

    #[test]
    fn basic_success() {
        test_case(
            "
            PUSHSLICE x779980_
            SREFS
        ",
        )
        .expect_item(int!(0));
    }
}

mod sbitrefs {
    use super::*;

    #[test]
    fn basic_success() {
        test_case(
            "
            PUSHSLICE x779980_
            SBITREFS
        ",
        )
        .expect_stack(Stack::new().push(int!(16)).push(int!(0)));
    }
}

mod ldzeroes {
    use super::*;

    #[test]
    fn basic_success_zero() {
        test_case(
            "
            PUSHSLICE x0000FF0080_
            LDZEROES
        ",
        )
        .expect_stack(Stack::new().push(int!(16)).push(create::slice([0xFF, 0x00, 0x80])));
    }
}

mod ldones {
    use super::*;

    #[test]
    fn basic_success_one() {
        test_case(
            "
            PUSHSLICE xFFFF00FF80_
            LDONES
        ",
        )
        .expect_stack(Stack::new().push(int!(16)).push(create::slice([0x00, 0xFF, 0x80])));
    }
}

mod ldsame {
    use super::*;

    #[test]
    fn basic_success_one() {
        test_case(
            "
            PUSHSLICE xFFFF00FF80_
            PUSHINT 1
            LDSAME
        ",
        )
        .expect_stack(Stack::new().push(int!(16)).push(create::slice([0x00, 0xFF, 0x80])));
    }

    #[test]
    fn basic_success_zero() {
        test_case(
            "
            PUSHSLICE x0000FF0080_
            PUSHINT 0
            LDSAME
        ",
        )
        .expect_stack(Stack::new().push(int!(16)).push(create::slice([0xFF, 0x00, 0x80])));
    }
}

mod schkbitsq {
    use super::*;

    #[test]
    fn basic_success() {
        test_case(
            "
            PUSHSLICE x779980_
            PUSHINT 8
            SCHKBITSQ
        ",
        )
        .expect_stack(Stack::new().push(boolean!(true)));
    }

    #[test]
    fn basic_failure() {
        test_case(
            "
            PUSHSLICE x8_
            PUSHINT 8
            SCHKBITSQ
        ",
        )
        .expect_stack(Stack::new().push(boolean!(false)));
    }
}

mod schkrefs {
    use super::*;
    // TODO: check test names

    #[test]
    fn basic_success() {
        test_case(
            "
            PUSHSLICE x779980_
            PUSHINT 4
            SCHKREFS
        ",
        )
        .expect_failure(ExceptionCode::CellUnderflow);
    }

    #[test]
    fn basic_failure() {
        test_case(
            "
            NEWC
            ENDC
            NEWC
            STREF
            ENDC
            CTOS
            ONE
            SCHKREFS
        ",
        )
        .expect_stack(&Stack::new());
    }
}

mod schkrefsq {
    use super::*;
    // TODO: check test names

    #[test]
    fn basic_success() {
        test_case(
            "
            PUSHSLICE x779980_
            PUSHINT 4
            SCHKREFSQ
        ",
        )
        .expect_stack(Stack::new().push(boolean!(false)));
    }

    #[test]
    fn basic_failure() {
        test_case(
            "
            NEWC
            ENDC
            NEWC
            STREF
            ENDC
            CTOS
            ONE
            SCHKREFSQ
        ",
        )
        .expect_stack(Stack::new().push(boolean!(true)));
    }
}

mod schkbitrefs {
    use super::*;

    #[test]
    fn basic_success() {
        test_case(
            "
            ONE
            NEWC
            NEWC
            ENDCST
            STSLICECONST x85
            ENDC
            CTOS
            PUSHINT 8
            PUSHINT 1
            SCHKBITREFS
        ",
        )
        .expect_int_stack(&[1]);
    }

    #[test]
    fn basic_failure_1() {
        test_case(
            "
            NEWC
            ENDC
            NEWC
            STREF
            ENDC
            CTOS
            PUSHINT 8
            PUSHINT 4
            SCHKBITREFS
        ",
        )
        .expect_failure(ExceptionCode::CellUnderflow);
    }

    #[test]
    fn basic_failure_2() {
        test_case(
            "
            PUSHSLICE x8_
            PUSHINT 8
            PUSHINT 4
            SCHKBITREFS
        ",
        )
        .expect_failure(ExceptionCode::CellUnderflow);
    }
}

mod schkbitrefsq {
    use super::*;

    #[test]
    fn basic_success() {
        test_case(
            "
            NEWC
            NEWC
            ENDCST
            STSLICECONST x85
            ENDC
            CTOS
            PUSHINT 8
            PUSHINT 1
            SCHKBITREFSQ
        ",
        )
        .expect_stack(Stack::new().push(boolean!(true)));
    }

    #[test]
    fn basic_failure_1() {
        test_case(
            "
            NEWC
            ENDC
            NEWC
            STREF
            STSLICECONST x85
            ENDC
            CTOS
            PUSHINT 8
            PUSHINT 4
            SCHKBITREFSQ
        ",
        )
        .expect_stack(Stack::new().push(boolean!(false)));
    }

    #[test]
    fn basic_failure_2() {
        test_case(
            "
            PUSHSLICE x85
            PUSHINT 8
            PUSHINT 4
            SCHKBITREFSQ
        ",
        )
        .expect_stack(Stack::new().push(boolean!(false)));
    }
}

mod sdbegins {
    use super::*;

    #[test]
    fn basic_scenarios() {
        test_case(
            "
            PUSHSLICE x11223344556677889980_
            SDBEGINS x112233445566778880_
        ",
        )
        .expect_stack(Stack::new().push(StackItem::Slice(SliceData::new(vec![0x99, 0x80]))));
    }

    #[test]
    fn same_slices() {
        test_case(
            "
            PUSHSLICE x779980_
            SDBEGINS x779980_
        ",
        )
        .expect_stack(Stack::new().push(StackItem::Slice(SliceData::new(vec![0x80]))));
    }

    #[test]
    fn no_prefix() {
        test_case(
            "
            PUSHSLICE x779980_
            SDBEGINS x1180_
        ",
        )
        .expect_failure(ExceptionCode::CellUnderflow);
    }

    #[test]
    fn too_short_slice() {
        test_case(
            "
            PUSHSLICE x779980_
            SDBEGINS x7779980_
        ",
        )
        .expect_failure(ExceptionCode::CellUnderflow);
    }
}

mod sdbegins_zero {
    use super::*;

    #[test]
    fn basic_scenarios() {
        test_case(
            "
            PUSHSLICE x7FFF80_
            SDBEGINS 0
        ",
        )
        .expect_stack(Stack::new().push(StackItem::Slice(SliceData::new(vec![0xFF, 0xFF]))));
    }

    #[test]
    fn no_prefix() {
        test_case(
            "
            PUSHSLICE xFF80_
            SDBEGINS 0
        ",
        )
        .expect_failure(ExceptionCode::CellUnderflow);
    }

    #[test]
    fn too_short_slice() {
        test_case(
            "
            PUSHSLICE x80_
            SDBEGINS 0
        ",
        )
        .expect_failure(ExceptionCode::CellUnderflow);
    }
}

mod sdbegins_one {
    use super::*;

    #[test]
    fn basic_scenarios() {
        test_case(
            "
            PUSHSLICE xFFFF80_
            SDBEGINS 1
        ",
        )
        .expect_stack(Stack::new().push(StackItem::Slice(SliceData::new(vec![0xFF, 0xFF]))));
    }

    #[test]
    fn no_prefix() {
        test_case(
            "
            PUSHSLICE x7F80_
            SDBEGINS 1
        ",
        )
        .expect_failure(ExceptionCode::CellUnderflow);
    }

    #[test]
    fn too_short_slice() {
        test_case(
            "
            PUSHSLICE x80_
            SDBEGINS 1
        ",
        )
        .expect_failure(ExceptionCode::CellUnderflow);
    }
}

mod sdbeginsq {
    use super::*;

    #[test]
    fn basic_scenarios() {
        test_case(
            "
            PUSHSLICE x11223344556677889980_
            SDBEGINSQ x112233445566778880_
        ",
        )
        .expect_stack(
            Stack::new()
                .push(StackItem::Slice(SliceData::new(vec![0x99, 0x80])))
                .push(boolean!(true)),
        );
    }

    #[test]
    fn same_slices() {
        test_case(
            "
            PUSHSLICE x779980_
            SDBEGINSQ x779980_
        ",
        )
        .expect_stack(
            Stack::new().push(StackItem::Slice(SliceData::new(vec![0x80]))).push(boolean!(true)),
        );
    }

    #[test]
    fn no_prefix() {
        test_case(
            "
            PUSHSLICE x779980_
            SDBEGINSQ x1180_
        ",
        )
        .expect_stack(
            Stack::new()
                .push(StackItem::Slice(SliceData::new(vec![0x77, 0x99, 0x80])))
                .push(boolean!(false)),
        );
    }

    #[test]
    fn too_short_slice() {
        test_case(
            "
            PUSHSLICE x80_
            SDBEGINSQ xFF80_
        ",
        )
        .expect_stack(
            Stack::new().push(StackItem::Slice(SliceData::new(vec![0x80]))).push(boolean!(false)),
        );
    }
}

mod sdbeginsxq {
    use super::*;

    #[test]
    fn basic_scenarios() {
        test_case(
            "
            PUSHSLICE x11223344556677889980_
            PUSHSLICE x112233445566778880_
            SDBEGINSXQ
        ",
        )
        .expect_stack(
            Stack::new()
                .push(StackItem::Slice(SliceData::new(vec![0x99, 0x80])))
                .push(boolean!(true)),
        );
    }

    #[test]
    fn same_slices() {
        test_case(
            "
            PUSHSLICE x779980_
            PUSHSLICE x779980_
            SDBEGINSXQ
        ",
        )
        .expect_stack(
            Stack::new().push(StackItem::Slice(SliceData::new(vec![0x80]))).push(boolean!(true)),
        );
    }

    #[test]
    fn no_prefix() {
        test_case(
            "
            PUSHSLICE x779980_
            PUSHSLICE x1180_
            SDBEGINSXQ
        ",
        )
        .expect_stack(
            Stack::new()
                .push(StackItem::Slice(SliceData::new(vec![0x77, 0x99, 0x80])))
                .push(boolean!(false)),
        );
    }

    #[test]
    fn too_short_slice() {
        test_case(
            "
            PUSHSLICE x80_
            PUSHSLICE xFF80_
            SDBEGINSXQ
        ",
        )
        .expect_stack(
            Stack::new().push(StackItem::Slice(SliceData::new(vec![0x80]))).push(boolean!(false)),
        );
    }
}

mod split {
    use super::*;

    #[test]
    fn basic_scenario() {
        test_case(
            "
            PUSHSLICE x779980_
            NEWC
            ENDC
            NEWC
            STREF
            STSLICE
            ENDC
            CTOS
            PUSHINT 8
            PUSHINT 1
            SPLIT
            SWAP
            LDREF
        ",
        )
        .expect_stack(
            Stack::new()
                .push(StackItem::Slice(SliceData::new(vec![0x99, 0x80])))
                .push(to_cell([0x80]))
                .push(StackItem::Slice(SliceData::new(vec![0x77, 0x80]))),
        );
    }

    #[test]
    fn only_by_bits() {
        test_case(
            "
            PUSHSLICE x779980_
            PUSHINT 16
            PUSHINT 0
            SPLIT
        ",
        )
        .expect_stack(
            Stack::new()
                .push(StackItem::Slice(SliceData::new(vec![0x77, 0x99, 0x80])))
                .push(StackItem::Slice(SliceData::new(vec![0x80]))),
        );
    }

    #[test]
    fn only_by_refs() {
        test_case(
            "
            PUSHSLICE x779980_
            NEWC
            ENDC
            NEWC
            STREF
            STSLICE
            ENDC
            CTOS
            PUSHINT 0
            PUSHINT 1
            SPLIT
            SWAP
            LDREF
        ",
        )
        .expect_stack(
            Stack::new()
                .push(StackItem::Slice(SliceData::new(vec![0x77, 0x99, 0x80])))
                .push(to_cell([0x80]))
                .push(StackItem::Slice(SliceData::new(vec![0x80]))),
        );
    }

    #[test]
    fn not_enough_bits() {
        test_case(
            "
            PUSHSLICE x779980_
            PUSHINT 17
            PUSHINT 0
            SPLIT
        ",
        )
        .expect_failure(ExceptionCode::CellUnderflow);
    }

    #[test]
    fn not_enough_refs() {
        test_case(
            "
            PUSHSLICE x779980_
            PUSHINT 0
            PUSHINT 1
            SPLIT
        ",
        )
        .expect_failure(ExceptionCode::CellUnderflow);
    }
}

mod splitq {
    use super::*;

    #[test]
    fn basic_scenario() {
        test_case(
            "
            PUSHSLICE x779980_
            NEWC
            ENDC
            NEWC
            STREF
            STSLICE
            ENDC
            CTOS
            PUSHINT 8
            PUSHINT 1
            SPLITQ
            PUSHCONT {
                SWAP
                LDREF
            }
            IF
        ",
        )
        .expect_stack(
            Stack::new()
                .push(StackItem::Slice(SliceData::new(vec![0x99, 0x80])))
                .push(to_cell([0x80]))
                .push(StackItem::Slice(SliceData::new(vec![0x77, 0x80]))),
        );
    }

    #[test]
    fn only_by_bits() {
        test_case(
            "
            PUSHSLICE x779980_
            PUSHINT 16
            PUSHINT 0
            SPLITQ
        ",
        )
        .expect_stack(
            Stack::new()
                .push(StackItem::Slice(SliceData::new(vec![0x77, 0x99, 0x80])))
                .push(StackItem::Slice(SliceData::new(vec![0x80])))
                .push(boolean!(true)),
        );
    }

    #[test]
    fn only_by_refs() {
        test_case(
            "
            PUSHSLICE x779980_
            NEWC
            ENDC
            NEWC
            STREF
            STSLICE
            ENDC
            CTOS
            PUSHINT 0
            PUSHINT 1
            SPLITQ
            PUSHCONT {
                SWAP
                LDREF
            }
            IF
        ",
        )
        .expect_stack(
            Stack::new()
                .push(StackItem::Slice(SliceData::new(vec![0x77, 0x99, 0x80])))
                .push(to_cell([0x80]))
                .push(StackItem::Slice(SliceData::new(vec![0x80]))),
        );
    }

    #[test]
    fn not_enough_bits() {
        test_case(
            "
            PUSHSLICE x779980_
            PUSHINT 17
            PUSHINT 0
            SPLITQ
        ",
        )
        .expect_stack(
            Stack::new()
                .push(StackItem::Slice(SliceData::new(vec![0x77, 0x99, 0x80])))
                .push(boolean!(false)),
        );
    }

    #[test]
    fn not_enough_refs() {
        test_case(
            "
            PUSHSLICE x779980_
            PUSHINT 0
            PUSHINT 1
            SPLITQ
        ",
        )
        .expect_stack(
            Stack::new()
                .push(StackItem::Slice(SliceData::new(vec![0x77, 0x99, 0x80])))
                .push(boolean!(false)),
        );
    }
}

mod scutfirst {
    use super::*;

    #[test]
    fn basic_scenarios() {
        test_case(
            "
            PUSHSLICE x11223344556677889980_
            PUSHINT 16
            PUSHINT 0
            SCUTFIRST
        ",
        )
        .expect_stack(Stack::new().push(StackItem::Slice(SliceData::new(vec![0x11, 0x22, 0x80]))));
    }
}

mod sskipfirst {
    use super::*;

    #[test]
    fn basic_scenarios() {
        test_case(
            "
            PUSHSLICE x11223344556677889980_
            PUSHINT 56
            PUSHINT 0
            SSKIPFIRST
        ",
        )
        .expect_stack(Stack::new().push(StackItem::Slice(SliceData::new(vec![0x88, 0x99, 0x80]))));
    }
}

mod scutlast {
    use super::*;

    #[test]
    fn basic_scenarios() {
        test_case(
            "
            PUSHSLICE x11223344556677889980_
            PUSHINT 16
            PUSHINT 0
            SCUTLAST
        ",
        )
        .expect_stack(Stack::new().push(StackItem::Slice(SliceData::new(vec![0x88, 0x99, 0x80]))));
    }
}

mod sskiplast {
    use super::*;

    #[test]
    fn basic_scenarios() {
        test_case(
            "
            PUSHSLICE x11223344556677889980_
            PUSHINT 56
            PUSHINT 0
            SSKIPLAST
        ",
        )
        .expect_stack(Stack::new().push(StackItem::Slice(SliceData::new(vec![0x11, 0x22, 0x80]))));
    }
}

mod subslice {
    use super::*;

    #[test]
    fn basic_scenarios() {
        test_case(
            "
            PUSHSLICE x11223344556677889980_
            PUSHINT 56
            PUSHINT 0
            PUSHINT 16
            PUSHINT 0
            SUBSLICE
        ",
        )
        .expect_stack(Stack::new().push(StackItem::Slice(SliceData::new(vec![0x88, 0x99, 0x80]))));
    }
}

mod pldref {
    use super::*;

    #[test]
    fn get_reference_from_slice() {
        test_case(
            "NEWC
             ENDC
             NEWC
             STREF
             ENDC
             CTOS
             PLDREF",
        )
        .expect_item(create::cell([0x80]));
    }

    #[test]
    fn get_reference_from_slice_run_quiet() {
        test_case(
            "NEWC
             ENDC
             NEWC
             STREFQ
             PUSHCONT {
                ENDC
                CTOS
                PLDREF
             }
             IFNOT",
        )
        .expect_item(create::cell([0x80]));
    }

    #[test]
    fn get_reference_from_slice_run_rev_quiet() {
        test_case(
            "NEWC
             ENDC
             NEWC
             SWAP
             STREFRQ
             PUSHCONT {
                ENDC
                CTOS
                PLDREF
             }
             IFNOT",
        )
        .expect_item(create::cell([0x80]));
    }

    #[test]
    fn get_reference_with_data_from_slice() {
        test_case(
            "PUSHINT 1
             NEWC
             STU 8
             ENDC
             NEWC
             STREF
             ENDC
             CTOS
             PLDREF",
        )
        .expect_item(create::cell([1, 0x80]));
    }

    #[test]
    fn get_reference_from_slice_err_no_ref() {
        test_case(
            "NEWC
             ENDC
             CTOS
             PLDREF",
        )
        .expect_failure(ExceptionCode::CellUnderflow);
    }

    #[test]
    fn get_reference_from_slice_err_no_slice() {
        test_case("PLDREF").expect_failure(ExceptionCode::StackUnderflow);
    }
}

mod pldrefvar {
    use super::*;

    #[test]
    fn get_first_reference_from_slice() {
        test_case(
            "NEWC
             ENDC
             NEWC
             STREF
             ENDC
             CTOS
             PUSHINT 0
             PLDREFVAR",
        )
        .expect_item(create::cell([0x80]));
    }

    #[test]
    fn get_second_reference_from_slice() {
        test_case(
            "PUSHINT 12
             NEWC
             STU 8
             ENDC
             NEWC
             ENDC

             NEWC
             STREF
             STREF
             ENDC
             CTOS
             PUSHINT 1
             PLDREFVAR",
        )
        .expect_item(create::cell([12, 0x80]));
    }

    #[test]
    fn get_third_reference_from_slice() {
        test_case(
            "PUSHINT 11
             NEWC
             STU 8
             ENDC
             NEWC
             ENDC
             NEWC
             ENDC

             NEWC
             STREF
             STREF
             STREF
             ENDC
             CTOS
             PUSHINT 2
             PLDREFVAR",
        )
        .expect_item(create::cell([11, 0x80]));
    }

    #[test]
    fn get_fourth_reference_from_slice() {
        test_case(
            "PUSHINT 10
             NEWC
             STU 8
             ENDC
             NEWC
             ENDC
             NEWC
             ENDC
             NEWC
             ENDC

             NEWC
             STREF
             STREF
             STREF
             STREF
             ENDC
             CTOS
             PUSHINT 3
             PLDREFVAR",
        )
        .expect_item(create::cell([10, 0x80]));
    }

    #[test]
    fn get_reference_from_slice_run_quiet() {
        test_case(
            "NEWC
             ENDC
             NEWC
             STREFQ
             PUSHCONT {
                ENDC
                CTOS
                PUSHINT 0
                PLDREFVAR
             }
             IFNOT",
        )
        .expect_item(create::cell([0x80]));
    }

    #[test]
    fn get_reference_from_slice_run_rev_quiet() {
        test_case(
            "NEWC
             ENDC
             NEWC
             SWAP
             STREFRQ
             PUSHCONT {
                ENDC
                CTOS
                PUSHINT 0
                PLDREFVAR
             }
             IFNOT",
        )
        .expect_item(create::cell([0x80]));
    }

    #[test]
    fn get_reference_with_data_from_slice() {
        test_case(
            "PUSHINT 1
             NEWC
             STU 8
             ENDC
             NEWC
             STREF
             ENDC
             CTOS
             PUSHINT 0
             PLDREFVAR",
        )
        .expect_item(create::cell([1, 0x80]));
    }

    #[test]
    fn get_reference_from_slice_err_no_ref() {
        test_case(
            "NEWC
             ENDC
             CTOS
             PUSHINT 0
             PLDREFVAR",
        )
        .expect_failure(ExceptionCode::CellUnderflow);
    }

    #[test]
    fn get_reference_from_slice_err_no_slice() {
        test_case("PLDREFVAR").expect_failure(ExceptionCode::StackUnderflow);
    }
}

mod pldrefidx {
    use super::*;

    #[test]
    fn get_first_reference_from_slice() {
        test_case(
            "NEWC
             ENDC
             NEWC
             STREF
             ENDC
             CTOS
             PLDREFIDX 0",
        )
        .expect_item(create::cell([0x80]));
    }

    #[test]
    fn get_second_reference_from_slice() {
        test_case(
            "PUSHINT 12
             NEWC
             STU 8
             ENDC
             NEWC
             ENDC

             NEWC
             STREF
             STREF
             ENDC
             CTOS
             PLDREFIDX 1",
        )
        .expect_item(create::cell([12, 0x80]));
    }

    #[test]
    fn get_third_reference_from_slice() {
        test_case(
            "PUSHINT 11
             NEWC
             STU 8
             ENDC
             NEWC
             ENDC
             NEWC
             ENDC

             NEWC
             STREF
             STREF
             STREF
             ENDC
             CTOS
             PLDREFIDX 2",
        )
        .expect_item(create::cell([11, 0x80]));
    }

    #[test]
    fn get_fourth_reference_from_slice() {
        test_case(
            "PUSHINT 10
             NEWC
             STU 8
             ENDC
             NEWC
             ENDC
             NEWC
             ENDC
             NEWC
             ENDC

             NEWC
             STREF
             STREF
             STREF
             STREF
             ENDC
             CTOS
             PLDREFIDX 3",
        )
        .expect_item(create::cell([10, 0x80]));
    }

    #[test]
    fn get_reference_from_slice_run_quiet() {
        test_case(
            "NEWC
             ENDC
             NEWC
             STREFQ
             PUSHCONT {
                ENDC
                CTOS
                PLDREFIDX 0
             }
             IFNOT",
        )
        .expect_item(create::cell([0x80]));
    }

    #[test]
    fn get_reference_from_slice_run_rev_quiet() {
        test_case(
            "NEWC
             ENDC
             NEWC
             SWAP
             STREFRQ
             PUSHCONT {
                ENDC
                CTOS
                PLDREFIDX 0
             }
             IFNOT",
        )
        .expect_item(create::cell([0x80]));
    }

    #[test]
    fn get_reference_with_data_from_slice() {
        test_case(
            "PUSHINT 1
             NEWC
             STU 8
             ENDC
             NEWC
             STREF
             ENDC
             CTOS
             PLDREFIDX 0",
        )
        .expect_item(create::cell([1, 0x80]));
    }

    #[test]
    fn get_reference_from_slice_err_no_ref() {
        test_case(
            "NEWC
             ENDC
             CTOS
             PLDREFIDX 0",
        )
        .expect_failure(ExceptionCode::CellUnderflow);
    }

    #[test]
    fn get_reference_from_slice_err_no_slice() {
        test_case("PLDREFIDX 0").expect_failure(ExceptionCode::StackUnderflow);
    }
}

mod datasize {
    use super::*;
    use ton_block::Cell;

    fn prepare_cell() -> Cell {
        let first = SliceData::new(vec![42, 0x80]).into_cell().unwrap();
        let second = SliceData::new(vec![17, 0x80]).into_cell().unwrap();
        BuilderData::with_raw_and_refs(vec![17], 8, vec![first.clone(), second, first])
            .unwrap()
            .into_cell()
            .unwrap()
    }

    #[test]
    fn cell_data_size_with_same_refs() {
        let code = "
            NEWC
            PUSHINT 17
            STUR 8
            DUP
            ENDC
            SWAP
            OVER
            STREFR
            OVER
            STREFR
            STREF
            ENDC
            PUSHINT 4
            CDATASIZE
        ";

        // main cell and same refs as one
        test_case(code).expect_gas(1000000000, 1000000000, 0, 999998495).expect_int_stack(&[
            2,
            8 * 2,
            3,
        ]);
    }

    #[test]
    fn cell_datasize_quite_null_normal() {
        test_case(
            "
            NULL
            ZERO
            CDATASIZEQ
        ",
        )
        .expect_int_stack(&[0, 0, 0, -1]);
    }

    #[test]
    fn cell_datasize_quite_normal() {
        test_case_with_ref(
            "
            PUSHREF
            TEN
            CDATASIZEQ
        ",
            prepare_cell(),
        )
        .expect_int_stack(&[3, 24, 3, -1]);

        test_case_with_ref(
            "
            PUSHREF
            PUSHPOW2DEC 256
            CDATASIZEQ
        ",
            prepare_cell(),
        )
        .expect_int_stack(&[3, 24, 3, -1]);
    }

    #[test]
    fn cell_datasize_quite_library() {
        let first = SliceData::new(vec![42, 0x80]).into_cell().unwrap();

        let mut b = BuilderData::default();
        b.set_type(CellType::LibraryReference);
        b.append_i8(2).unwrap();
        b.append_u128(0).unwrap();
        b.append_u128(0).unwrap();
        let second = b.into_cell().unwrap();

        let cell = BuilderData::with_raw_and_refs(vec![17], 8, vec![first.clone(), second, first])
            .unwrap()
            .into_cell()
            .unwrap();

        test_case_with_ref(
            "
            PUSHREF
            TEN
            CDATASIZEQ
        ",
            cell,
        )
        .expect_int_stack(&[3, 280, 3, -1]);
    }

    #[test]
    fn cell_datasize_quite_stack_underflow() {
        expect_exception("CDATASIZEQ", ExceptionCode::StackUnderflow);
        expect_exception("NULL CDATASIZEQ", ExceptionCode::StackUnderflow);
    }

    #[test]
    fn cell_datasize_quite_range_check_error() {
        expect_exception("NULL PUSHINT -1 CDATASIZEQ", ExceptionCode::RangeCheckError);
    }

    #[test]
    fn cell_datasize_quite_wrong_type() {
        expect_exception("NULL NULL CDATASIZEQ", ExceptionCode::TypeCheckError);
        expect_exception(
            "
            PUSHSLICE x_
            ZERO
            CDATASIZEQ
        ",
            ExceptionCode::TypeCheckError,
        );
        expect_exception(
            "
            NEWC
            ENDC
            PUSHSLICE x_
            CDATASIZEQ
        ",
            ExceptionCode::TypeCheckError,
        );
    }

    #[test]
    fn cell_datasize_quite_limit_error() {
        test_case_with_ref(
            "
            PUSHREF
            ONE
            CDATASIZEQ
        ",
            prepare_cell(),
        )
        .expect_int_stack(&[0]);
    }

    #[test]
    fn cell_datasize_null_normal() {
        test_case(
            "
            NULL
            ZERO
            CDATASIZE
        ",
        )
        .expect_int_stack(&[0, 0, 0]);
    }

    #[test]
    fn cell_datasize_normal() {
        test_case_with_ref(
            "
            PUSHREF
            TEN
            CDATASIZE
        ",
            prepare_cell(),
        )
        .expect_int_stack(&[3, 24, 3]);

        test_case_with_ref(
            "
            PUSHREF
            PUSHPOW2DEC 256
            CDATASIZE
        ",
            prepare_cell(),
        )
        .expect_int_stack(&[3, 24, 3]);
    }

    #[test]
    fn cell_datasize_stack_underflow() {
        expect_exception("CDATASIZE", ExceptionCode::StackUnderflow);
        expect_exception("NULL CDATASIZE", ExceptionCode::StackUnderflow);
    }

    #[test]
    fn cell_datasize_range_check_error() {
        expect_exception("NULL PUSHINT -1 CDATASIZE", ExceptionCode::RangeCheckError);
    }

    #[test]
    fn cell_datasize_wrong_type() {
        expect_exception("NULL NULL CDATASIZE", ExceptionCode::TypeCheckError);
        expect_exception(
            "
            PUSHSLICE x_
            ZERO
            CDATASIZE
        ",
            ExceptionCode::TypeCheckError,
        );
        expect_exception(
            "
            NEWC
            ENDC
            PUSHSLICE x_
            CDATASIZE
        ",
            ExceptionCode::TypeCheckError,
        );
    }

    #[test]
    fn cell_datasize_limit_error() {
        test_case_with_ref(
            "
            PUSHREF
            ONE
            CDATASIZE
        ",
            prepare_cell(),
        )
        .expect_failure(ExceptionCode::CellOverflow);
    }

    #[test]
    fn slice_datasize_quite_empty_normal() {
        test_case_with_ref(
            "
            PUSHREFSLICE
            ZERO
            SDATASIZEQ
        ",
            Cell::default(),
        )
        .expect_int_stack(&[0, 0, 0, -1]);
    }

    #[test]
    fn slice_datasize_quite_simple_normal() {
        test_case(
            "
            PUSHSLICE x123456
            ZERO
            SDATASIZEQ
        ",
        )
        .expect_int_stack(&[0, 24, 0, -1]);
    }

    #[test]
    fn slice_datasize_quite_normal() {
        test_case_with_ref(
            "
            PUSHREFSLICE
            TEN
            SDATASIZEQ
        ",
            prepare_cell(),
        )
        .expect_int_stack(&[2, 24, 3, -1]);

        test_case_with_ref(
            "
            PUSHREFSLICE
            PUSHPOW2DEC 256
            SDATASIZEQ
        ",
            prepare_cell(),
        )
        .expect_int_stack(&[2, 24, 3, -1]);
    }

    #[test]
    fn slice_datasize_quite_type_check_error() {
        expect_exception("NULL PUSHINT -1 SDATASIZEQ", ExceptionCode::TypeCheckError);
    }

    #[test]
    fn slice_datasize_quite_range_check_error() {
        expect_exception("PUSHSLICE x0 PUSHINT -1 SDATASIZEQ", ExceptionCode::RangeCheckError);
    }

    #[test]
    fn slice_datasize_empty_normal() {
        test_case_with_ref(
            "
            PUSHREFSLICE
            ZERO
            SDATASIZE
        ",
            Cell::default(),
        )
        .expect_int_stack(&[0, 0, 0]);
    }

    #[test]
    fn slice_datasize_simple_normal() {
        test_case(
            "
            PUSHSLICE x123456
            ZERO
            SDATASIZE
        ",
        )
        .expect_int_stack(&[0, 24, 0]);
    }

    #[test]
    fn slice_datasize_normal() {
        test_case_with_ref(
            "
            PUSHREFSLICE
            TEN
            SDATASIZE
        ",
            prepare_cell(),
        )
        .expect_int_stack(&[2, 24, 3]);

        test_case_with_ref(
            "
            PUSHREFSLICE
            PUSHPOW2DEC 256
            SDATASIZE
        ",
            prepare_cell(),
        )
        .expect_int_stack(&[2, 24, 3]);
    }

    #[test]
    fn slice_datasize_stack_underflow() {
        expect_exception("SDATASIZE", ExceptionCode::StackUnderflow);
        expect_exception("NULL SDATASIZE", ExceptionCode::StackUnderflow);
    }

    #[test]
    fn slice_datasize_wrong_type() {
        expect_exception("NULL NULL SDATASIZE", ExceptionCode::TypeCheckError);
        expect_exception(
            "
            NULL
            ZERO
            SDATASIZE
        ",
            ExceptionCode::TypeCheckError,
        );
        expect_exception(
            "
            PUSHSLICE x_
            PUSHSLICE x_
            SDATASIZE
        ",
            ExceptionCode::TypeCheckError,
        );
    }

    #[test]
    fn slice_datasize_limit_error() {
        test_case_with_ref(
            "
            PUSHREFSLICE
            ONE
            SDATASIZE
        ",
            prepare_cell(),
        )
        .expect_failure(ExceptionCode::CellOverflow);
    }

    #[test]
    fn slice_datasize_type_check_error() {
        expect_exception("NULL PUSHINT -1 SDATASIZE", ExceptionCode::TypeCheckError);
    }

    #[test]
    fn slice_datasize_range_check_error() {
        expect_exception("PUSHSLICE x0 PUSHINT -1 SDATASIZE", ExceptionCode::RangeCheckError);
    }
}

mod ends {
    use super::*;

    #[test]
    fn test_ends_full_empty_cell() {
        test_case(
            "
            NEWC ENDC
            NEWC STREF ENDC
            CTOS ENDS
        ",
        )
        .expect_failure(ExceptionCode::CellUnderflow);
    }
}
