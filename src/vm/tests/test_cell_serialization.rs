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
use ton_block::{BuilderData, ExceptionCode, SliceData};
use ton_vm::{
    int,
    stack::{integer::IntegerData, Stack, StackItem},
};

mod common;
use common::*;

const PUSHINT1: [u8; 2] = [0x71, 0x80];
const PUSHINT2: [u8; 2] = [0x72, 0x80];
const PUSHINT3: [u8; 2] = [0x73, 0x80];

fn pushpow2_255i_vector(data: &mut Vec<u8>) {
    data.push(0x7f);
    for _b in 0..31 {
        data.push(0xff);
    }
}

mod newc {
    use super::*;

    #[test]
    fn create_builder() {
        test_case("NEWC").expect_item(create::builder([0x80]));
    }
}

mod endc {
    use super::*;

    #[test]
    fn create_cell() {
        test_case(
            "NEWC
             ENDC",
        )
        .expect_bytecode(vec![0xc8, 0xc9, 0x80])
        .expect_item(create::cell([0x80]));
    }

    #[test]
    fn create_cell_err_no_builder() {
        test_case("ENDC").expect_failure(ExceptionCode::StackUnderflow);
    }
}

mod stbq {
    use super::*;

    #[test]
    fn put_builder_with_data_quiet_to_builder() {
        test_case(
            "NEWC
             NEWC
             STBQ
             DROP",
        )
        .expect_item(create::builder([0x80]));

        test_case(
            "PUSHINT 1
             NEWC
             STU 8
             NEWC
             STBQ
             DROP",
        )
        .expect_item(create::builder([1, 0x80]));

        test_case(
            "PUSHINT 1
             NEWC
             STU 8
             PUSHINT 2
             NEWC
             STU 8
             STBQ
             DROP",
        )
        .expect_item(create::builder([2, 1, 0x80]));

        let mut data = Vec::<u8>::new();
        pushpow2_255i_vector(&mut data);
        pushpow2_255i_vector(&mut data);
        pushpow2_255i_vector(&mut data);
        data.push(0x80);
        let mut data1 = vec![0xff; 96];
        data1.push(0x80);
        test_case(
            "PUSHPOW2 255
             ADDCONST -1
             DUP
             DUP
             NEWC
             STI 256
             STI 256
             STI 256
             PUSHPOW2 255
             DUP
             PUSHINT -1
             ADD
             ADD
             DUP
             DUP
             NEWC
             STU 256
             STU 256
             STU 256
             STBQ",
        )
        .expect_stack(
            Stack::new().push(create::builder(data)).push(create::builder(data1)).push(int!(-1)),
        );
    }
    #[test]
    fn put_builder_with_data_quiet_to_builder_failure() {
        test_case("STBQ").expect_failure(ExceptionCode::StackUnderflow);

        test_case(
            "NEWC
             STBQ",
        )
        .expect_failure(ExceptionCode::StackUnderflow);

        test_case(
            "NEWC
             PUSHINT 1
             STBQ",
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }
}

mod stbrq {
    use super::*;

    #[test]
    fn put_builder_with_data_quiet_rev_to_builder() {
        test_case(
            "PUSHINT 1
             NEWC
             STU 8
             NEWC
             SWAP
             STBRQ
             DROP",
        )
        .expect_item(create::builder([1, 0x80]));
    }
}

mod stb {
    use super::*;

    #[test]
    fn put_builder_to_builder() {
        test_case(
            "NEWC
             NEWC
             STB",
        )
        .expect_item(create::builder([0x80]));
    }

    #[test]
    fn put_builder_with_data_to_builder() {
        test_case(
            "PUSHINT 1
             NEWC
             STU 8
             NEWC
             STB",
        )
        .expect_item(create::builder([1, 0x80]));

        let mut data = vec![0xff; 32];
        data.push(0x80);
        test_case(
            "PUSHPOW2 255
             PUSHPOW2 255
             ADDCONST -1
             ADD
             NEWC
             STU 256
             NEWC
             STB",
        )
        .expect_stack(Stack::new().push(create::builder(data)));
    }

    #[test]
    fn put_builder_to_builder_err_no_builder() {
        test_case(
            "NEWC
             STB",
        )
        .expect_failure(ExceptionCode::StackUnderflow);
    }
    #[test]
    fn put_builder_to_builder_type_err() {
        test_case(
            "NEWC
             PUSHINT 1
             STB",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT 1
             NEWC
             STB",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             PUSHSLICE x8_
             STB",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHSLICE x8_
             NEWC
             STB",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             NEWC
             ENDC
             STB",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             ENDC
             NEWC
             STB",
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }
    #[test]
    fn put_builder_to_builder_over() {
        test_case(
            "PUSHPOW2 255
             PUSHPOW2 255
             PUSHPOW2 255
             NEWC
             STU 256
             STU 256
             STU 256
             PUSHPOW2 255
             PUSHPOW2 255
             PUSHPOW2 255
             NEWC
             STU 256
             STU 256
             STU 256
             STB",
        )
        .expect_failure(ExceptionCode::CellOverflow);
    }
}

mod stbrefq {
    use super::*;

    #[test]
    fn put_builder_as_ref_to_builder_quiet() {
        let mut builder = BuilderData::new();
        builder.checked_append_reference(BuilderData::new().into_cell().unwrap()).unwrap();
        test_case(
            "NEWC
             NEWC
             STBREFQ
             DROP",
        )
        .expect_item(StackItem::builder(builder));
    }
    #[test]
    fn put_builder_as_ref_to_builder_quiet_err() {
        test_case(
            "NEWC
             ENDC
             NEWC
             STBREFQ
             DROP",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             NEWC
             ENDC
             STBREFQ
             DROP",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT 1
             NEWC
             STBREFQ
             DROP",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             PUSHINT 1
             STBREFQ
             DROP",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHSLICE x8_
             NEWC
             STBREFQ
             DROP",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             PUSHSLICE x8_
             STBREFQ
             DROP",
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }
}

mod stbrefrq {
    use super::*;

    #[test]
    fn put_builder_as_ref_to_builder_rev_quiet() {
        let mut builder = BuilderData::new();
        builder.checked_append_reference(BuilderData::new().into_cell().unwrap()).unwrap();
        test_case(
            "NEWC
             NEWC
             STBREFRQ
             DROP",
        )
        .expect_item(StackItem::builder(builder));
    }
    #[test]
    fn put_builder_as_ref_to_builder_rev_quiet_err() {
        test_case(
            "NEWC
             ENDC
             NEWC
             STBREFRQ
             DROP",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             NEWC
             ENDC
             STBREFRQ
             DROP",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT 1
             NEWC
             STBREFRQ
             DROP",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             PUSHINT 1
             STBREFRQ
             DROP",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHSLICE x8_
             NEWC
             STBREFRQ
             DROP",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             PUSHSLICE x8_
             STBREFRQ
             DROP",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             STBREFRQ
             DROP",
        )
        .expect_failure(ExceptionCode::StackUnderflow);
    }
}

mod stbref {
    use super::*;

    #[test]
    fn put_builder_as_ref_to_builder() {
        let mut builder = BuilderData::new();
        builder.checked_append_reference(BuilderData::new().into_cell().unwrap()).unwrap();
        test_case(
            "NEWC
             NEWC
             STBREF",
        )
        .expect_item(StackItem::builder(builder));
    }

    #[test]
    fn put_builder_as_ref_with_data_to_builder() {
        let mut builder = BuilderData::new();
        builder
            .checked_append_reference(SliceData::new(vec![1, 0x80]).into_cell().unwrap())
            .unwrap();
        test_case(
            "PUSHINT 1
             NEWC
             STU 8
             NEWC
             STBREF",
        )
        .expect_item(StackItem::builder(builder));
    }

    #[test]
    fn put_builder_as_ref_to_builder_err_no_builder() {
        test_case(
            "NEWC
             STBREF",
        )
        .expect_failure(ExceptionCode::StackUnderflow);
    }

    #[test]
    fn put_builder_as_ref_to_builder_err_type() {
        test_case(
            "NEWC
             PUSHINT 1
             STBREF",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT 1
             NEWC
             STBREF",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             PUSHSLICE x8_
             STBREF",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHSLICE x8_
             NEWC
             STBREF",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             NEWC
             ENDC
             STBREF",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             ENDC
             NEWC
             STBREF",
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }
}

mod endcst {
    use super::*;

    #[test]
    fn put_builder_as_cell_to_builder_normal() {
        let mut builder = BuilderData::new();
        builder.checked_append_reference(BuilderData::new().into_cell().unwrap()).unwrap();
        let stack_item = StackItem::builder(builder);
        test_case(
            "NEWC
             NEWC
             ENDCST",
        )
        .expect_item(stack_item.clone());

        // test STBREFR alias
        test_case(
            "NEWC
             NEWC
             STBREFR",
        )
        .expect_item(stack_item);
    }

    #[test]
    fn put_builder_as_cell_with_data_to_builder2() {
        let mut builder = SliceData::new(vec![1, 0x80]).into_builder().unwrap();
        builder.checked_append_reference(BuilderData::new().into_cell().unwrap()).unwrap();
        test_case(
            "PUSHINT 1
             NEWC
             STU 8
             NEWC
             ENDCST",
        )
        .expect_item(StackItem::builder(builder));
    }

    #[test]
    fn put_builder_as_cell_to_builder_max_ref() {
        let mut builder = BuilderData::new();
        builder.checked_append_reference(BuilderData::new().into_cell().unwrap()).unwrap();
        builder.checked_append_reference(BuilderData::new().into_cell().unwrap()).unwrap();
        builder.checked_append_reference(BuilderData::new().into_cell().unwrap()).unwrap();
        builder.checked_append_reference(BuilderData::new().into_cell().unwrap()).unwrap();
        test_case(
            "NEWC
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
             SWAP
             ENDCST",
        )
        .expect_item(StackItem::builder(builder));
    }

    #[test]
    fn put_builder_as_cell_to_builder_err_no_builder2() {
        test_case(
            "NEWC
             ENDCST",
        )
        .expect_failure(ExceptionCode::StackUnderflow);
    }

    #[test]
    fn put_builder_as_cell_to_builder_err_type() {
        test_case(
            "PUSHINT 1
             NEWC
             ENDCST",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHSLICE x8_
             NEWC
             ENDCST",
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }

    #[test]
    fn put_builder_as_cell_to_builder_err_ref_overflow() {
        test_case(
            "NEWC
             NEWC
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
             SWAP
             ENDCST",
        )
        .expect_failure(ExceptionCode::CellOverflow);
    }
}

mod sti {
    use super::*;

    #[test]
    fn put_sign_number_to_builder() {
        test_case(
            "PUSHINT 1
             NEWC
             STI 5
             PUSHINT -1
             NEWC
             STI 6",
        )
        .expect_stack(Stack::new().push(create::builder([0x0C])).push(create::builder([0xFE])));
    }

    #[test]
    fn get_max_255_bits() {
        let mut vec = vec![0x00; 31];
        vec.push(0x03);

        test_case(
            "PUSHINT 1
             NEWC
             STI 255",
        )
        .expect_stack(Stack::new().push(create::builder(vec)));
    }

    #[test]
    fn get_max_n_of_bits() {
        let mut vec = vec![0x00; 31];
        vec.push(0x01);
        vec.push(0x80);

        test_case(
            "PUSHINT 1
             NEWC
             STI 256",
        )
        .expect_stack(Stack::new().push(create::builder(vec)));
    }

    #[test]
    fn get_over_max_n_of_bits() {
        test_case(
            "PUSHINT 1
             NEWC
             STI 257",
        )
        .expect_compilation_failure(CompileError::out_of_range(3, 14, "STI", "arg 0"));
    }

    #[test]
    fn spec_example_page_34_remark_1() {
        test_case(
            "PUSHINT -17
             NEWC
             STI 8",
        )
        .expect_stack(Stack::new().push(create::builder([0xEF, 0x80])));
    }
}

mod stiq {
    use super::*;

    #[test]
    fn put_sign_number_quiet_to_builder() {
        test_case(
            "PUSHINT 1
             NEWC
             STIQ 5
             PUSHINT -1
             NEWC
             STIQ 6",
        )
        .expect_stack(
            Stack::new()
                .push(create::builder([0x0C]))
                .push(int!(0))
                .push(create::builder([0xFE]))
                .push(int!(0)),
        );
    }

    #[test]
    fn put_sign_number_quiet_to_builder_over() {
        let mut vec = vec![0xFF; 96];
        vec.push(0x80);
        test_case(
            "PUSHINT -1
             DUP
             DUP
             DUP
             NEWC
             STI 256
             STI 256
             STI 256
             STIQ 256",
        )
        .expect_stack(Stack::new().push(int!(-1)).push(create::builder(vec)).push(int!(-1)));
    }
}

mod stuq {
    use super::*;

    #[test]
    fn put_unsign_number_quiet_to_builder() {
        test_case(
            "PUSHINT 1
             NEWC
             STUQ 5
             DROP",
        )
        .expect_item(create::builder([0x0C]));
    }

    #[test]
    fn put_unsign_number_quiet_to_builder_err_wrong_integer() {
        test_case(
            "
            PUSHINT 256
            NEWC
            STUQ 8
        ",
        )
        .expect_stack(Stack::new().push(int!(256)).push_builder(BuilderData::new()).push(int!(1)));
    }

    #[test]
    fn put_unsign_number_quiet_to_builder_over() {
        let mut vec = vec![0xFF; 96];
        vec.push(0x80);
        test_case(
            "PUSHINT 1
             PUSHPOW2 255
             PUSHPOW2 255
             ADDCONST -1
             ADD
             DUP
             DUP
             NEWC
             STU 256
             STU 256
             STU 256
             STUQ 256",
        )
        .expect_stack(Stack::new().push(int!(1)).push(create::builder(vec)).push(int!(-1)));
    }
}

mod stirq {
    use super::*;

    #[test]
    fn put_sign_number_quiet_rev_to_builder() {
        test_case(
            "NEWC
             PUSHINT -1
             STIRQ 6",
        )
        .expect_stack(Stack::new().push(create::builder([0xFE])).push(int!(0)));
    }

    #[test]
    fn put_nan_value_quiet_rev_to_builder() {
        test_case(
            "NEWC
             PUSHNAN
             STIRQ 1",
        )
        .expect_stack(Stack::new().push(create::builder([0x80])).push(int!(nan)).push(int!(1)));

        test_case(
            "NEWC
             PUSHNAN
             STIRQ 6",
        )
        .expect_stack(Stack::new().push(create::builder([0x80])).push(int!(nan)).push(int!(1)));

        test_case(
            "NEWC
             PUSHNAN
             STIRQ 256",
        )
        .expect_stack(Stack::new().push(create::builder([0x80])).push(int!(nan)).push(int!(1)));
    }

    #[test]
    fn put_bigint_number_quiet_rev_to_builder() {
        test_case(
            "NEWC
             PUSHPOW2 256
             STIRQ 256
            ",
        )
        .expect_stack(Stack::new().push(create::builder([0x80])).push(int!(nan)).push(int!(1)));

        let mut data = vec![0x7f];
        for _b in 0..31 {
            data.push(0xff);
        }
        data.push(0x80);
        test_case(
            "NEWC
             PUSHPOW2 255
             ADDCONST -1
             STIRQ 256
            ",
        )
        .expect_stack(Stack::new().push(create::builder(data)).push(int!(0)));

        data = vec![0x80];
        for _b in 0..31 {
            data.push(0x00);
        }
        data.push(0x80);
        test_case(
            "NEWC
             PUSHPOW2 255
             MULCONST -1
             STIRQ 256
            ",
        )
        .expect_stack(Stack::new().push(create::builder(data)).push(int!(0)));
    }
}

mod sturq {
    use super::*;

    #[test]
    fn put_unsign_number_quiet_rev_to_builder() {
        test_case(
            "NEWC
             PUSHINT 1
             STURQ 8
             DROP",
        )
        .expect_item(create::builder([1, 0x80]));
    }
    #[test]
    fn put_nan_value_quiet_rev_to_builder() {
        test_case(
            "NEWC
             PUSHNAN
             STURQ 1",
        )
        .expect_stack(Stack::new().push(create::builder([0x80])).push(int!(nan)).push(int!(1)));

        test_case(
            "NEWC
             PUSHNAN
             STURQ 6",
        )
        .expect_stack(Stack::new().push(create::builder([0x80])).push(int!(nan)).push(int!(1)));

        test_case(
            "NEWC
             PUSHNAN
             STURQ 256",
        )
        .expect_stack(Stack::new().push(create::builder([0x80])).push(int!(nan)).push(int!(1)));
    }

    #[test]
    fn put_biguint_number_quiet_rev_to_builder() {
        test_case(
            "NEWC
             PUSHPOW2 256
             STURQ 256
            ",
        )
        .expect_stack(Stack::new().push(create::builder([0x80])).push(int!(nan)).push(int!(1)));

        let mut data = Vec::new();
        for _b in 0..32 {
            data.push(0xFF);
        }
        data.push(0x80);
        test_case(
            "NEWC
             PUSHPOW2 255
             PUSHPOW2 255
             PUSHINT 1
             SUB
             ADD
             STURQ 256
            ",
        )
        .expect_stack(Stack::new().push(create::builder(data)).push(int!(0)));
    }
}

mod stir {
    use super::*;

    #[test]
    fn put_sign_number_rev_to_builder() {
        test_case(
            "NEWC
             PUSHINT -1
             STIR 6",
        )
        .expect_item(create::builder([0xFE]));
    }

    #[test]
    fn put_sign_number_rev_to_builder_err() {
        test_case(
            "NEWC
             STIR 6",
        )
        .expect_failure(ExceptionCode::StackUnderflow);

        test_case(
            "NEWC
             PUSHINT 65536
             STIR 1",
        )
        .expect_failure(ExceptionCode::RangeCheckError);

        test_case(
            "NEWC
             PUSHSLICE x8_
             STIR 1",
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }
}

mod stur {
    use super::*;

    #[test]
    fn put_unsign_number_rev_to_builder() {
        test_case(
            "NEWC
             PUSHINT 1
             STUR 8",
        )
        .expect_item(create::builder([1, 0x80]));
    }

    #[test]
    fn put_sign_number_rev_to_builder_err() {
        test_case(
            "NEWC
             STUR 6",
        )
        .expect_failure(ExceptionCode::StackUnderflow);

        test_case(
            "NEWC
             PUSHINT 65536
             STUR 1",
        )
        .expect_failure(ExceptionCode::RangeCheckError);

        test_case(
            "NEWC
             PUSHSLICE x8_
             STUR 1",
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }
}

mod stix {
    use super::*;

    #[test]
    fn put_sign_len_number_to_builder_success() {
        test_case(
            "PUSHINT -1
             NEWC
             PUSHINT 30
             STIX",
        )
        .expect_item(create::builder([0xFF, 0xFF, 0xFF, 0xFE]));

        let mut data = vec![0xff; 32];
        data.push(0x80);

        test_case(
            "PUSHINT -1
             NEWC
             PUSHINT 256
             STIX",
        )
        .expect_item(create::builder(data.clone()));

        let mut builder = BuilderData::new();
        builder.append_raw(data.as_slice(), 255).unwrap();
        builder.append_raw(data.as_slice(), 256).unwrap();
        builder.append_raw(data.as_slice(), 256).unwrap();
        builder.append_raw(data.as_slice(), 256).unwrap();

        test_case(
            "PUSHINT -1
             PUSHINT -1
             PUSHINT -1
             PUSHINT -1
             NEWC
             STI 255
             STI 256
             STI 256
             PUSHINT 256
             STIX",
        )
        .expect_item(StackItem::builder(builder));
    }

    #[test]
    fn put_sign_len_number_to_builder_type_error() {
        test_case(
            "PUSHSLICE x8_
             NEWC
             PUSHINT 30
             STIX",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT -1
             NEWC
             PUSHSLICE x8_
             STIX",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             NEWC
             PUSHINT 30
             STIX",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT -1
             NEWC
             NEWC
             STIX",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             ENDC
             NEWC
             PUSHINT 30
             STIX",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT -1
             NEWC
             NEWC
             ENDC
             STIX",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT -1
             NEWC
             ENDC
             PUSHINT 8
             STIX",
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }

    #[test]
    fn put_sign_len_number_to_builder_underflow_error() {
        test_case(
            "NEWC
             PUSHINT 30
             STIX",
        )
        .expect_failure(ExceptionCode::StackUnderflow);

        test_case(
            "PUSHINT -1
             NEWC
             STIX",
        )
        .expect_failure(ExceptionCode::StackUnderflow);
    }

    #[test]
    fn put_sign_len_number_to_builder_range_error() {
        test_case(
            "PUSHINT -1
             NEWC
             PUSHINT -1
             STIX",
        )
        .expect_failure(ExceptionCode::RangeCheckError);

        test_case(
            "PUSHINT -1
             NEWC
             PUSHINT 258
             STIX",
        )
        .expect_failure(ExceptionCode::RangeCheckError);
    }
    #[test]
    fn put_sign_len_number_to_builder_cell_error() {
        test_case(
            "PUSHINT -1
             PUSHINT -1
             PUSHINT -1
             PUSHINT -1
             NEWC
             STI 256
             STI 256
             STI 256
             PUSHINT 256
             STIX",
        )
        .expect_failure(ExceptionCode::CellOverflow);
    }
}

mod stixr {
    use super::*;

    #[test]
    fn put_sign_len_number_rev_to_builder_with_a_remaining_element() {
        test_case(
            "PUSHINT 2
             NEWC
             PUSHINT -1
             PUSHINT 6
             STIXR",
        )
        .expect_stack(Stack::new().push(int!(2)).push(create::builder([0xFE])));
    }

    #[test]
    fn put_sign_len_number_rev_to_builder() {
        test_case(
            "NEWC
             PUSHINT 1
             PUSHINT 8
             STIXR",
        )
        .expect_item(create::builder([1, 0x80]));
    }

    #[test]
    fn put_sign_len_number_to_builder_type_error() {
        test_case(
            "NEWC
             PUSHSLICE x8_
             PUSHINT 30
             STIXR",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             PUSHINT -1
             PUSHSLICE x8_
             STIXR",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             NEWC
             PUSHINT 30
             STIXR",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             PUSHINT -1
             NEWC
             STIXR",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             NEWC
             ENDC
             PUSHINT 30
             STIXR",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             PUSHINT -1
             NEWC
             ENDC
             STIXR",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             ENDC
             PUSHINT -1
             PUSHINT 8
             STIXR",
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }

    #[test]
    fn put_sign_len_number_to_builder_underflow_error() {
        test_case(
            "NEWC
             PUSHINT 30
             STIXR",
        )
        .expect_failure(ExceptionCode::StackUnderflow);

        test_case(
            "NEWC
             STIXR",
        )
        .expect_failure(ExceptionCode::StackUnderflow);
    }

    #[test]
    fn put_sign_len_number_to_builder_range_error() {
        test_case(
            "NEWC
             PUSHINT -1
             PUSHINT -1
             STIXR",
        )
        .expect_failure(ExceptionCode::RangeCheckError);

        test_case(
            "NEWC
             PUSHINT -1
             PUSHINT 258
             STIXR",
        )
        .expect_failure(ExceptionCode::RangeCheckError);
    }

    #[test]
    fn put_sign_len_number_to_builder_cell_error() {
        test_case(
            "PUSHINT -1
             PUSHINT -1
             PUSHINT -1
             NEWC
             STI 256
             STI 256
             STI 256
             PUSHINT -1
             PUSHINT 256
             STIXR",
        )
        .expect_failure(ExceptionCode::CellOverflow);
    }
}

mod stixq {
    use super::*;

    #[test]
    fn put_sign_number_to_builder_success() {
        test_case(
            "PUSHINT -1
             NEWC
             PUSHINT 30
             STIXQ",
        )
        .expect_stack(Stack::new().push(create::builder([0xFF, 0xFF, 0xFF, 0xFE])).push(int!(0)));

        let mut data = vec![0xff; 32];
        data.push(0x80);
        let mut builder = BuilderData::new();
        builder.append_raw(data.as_slice(), 256).unwrap();
        builder.append_raw(data.as_slice(), 256).unwrap();
        builder.append_raw(data.as_slice(), 256).unwrap();

        test_case(
            "PUSHINT -1
             PUSHINT -1
             PUSHINT -1
             PUSHINT -1
             NEWC
             STI 256
             STI 256
             STI 256
             PUSHINT 256
             STIXQ",
        )
        .expect_stack(Stack::new().push(int!(-1)).push(StackItem::builder(builder)).push(int!(-1)));
    }
    #[test]
    fn put_sign_number_to_builder_type_error() {
        test_case(
            "PUSHSLICE x8_
             NEWC
             PUSHINT 30
             STIXQ",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT -1
             NEWC
             PUSHSLICE x8_
             STIXQ",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             NEWC
             PUSHINT 30
             STIXQ",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT -1
             NEWC
             NEWC
             STIXQ",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             ENDC
             NEWC
             PUSHINT 30
             STIXQ",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT -1
             NEWC
             NEWC
             ENDC
             STIXQ",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT -1
             NEWC
             ENDC
             PUSHINT 8
             STIXQ",
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }

    #[test]
    fn put_sign_number_to_builder_underflow_error() {
        test_case(
            "PUSHINT 30
             NEWC
             STIXQ",
        )
        .expect_failure(ExceptionCode::StackUnderflow);

        test_case(
            "NEWC
             STIXQ",
        )
        .expect_failure(ExceptionCode::StackUnderflow);
    }

    #[test]
    fn put_sign_number_to_builder_range_error() {
        test_case(
            "PUSHINT -1
             NEWC
             PUSHINT -1
             STIXQ",
        )
        .expect_failure(ExceptionCode::RangeCheckError);

        test_case(
            "PUSHINT -1
             NEWC
             PUSHINT 258
             STIXQ",
        )
        .expect_failure(ExceptionCode::RangeCheckError);
    }
}

mod stuxq {
    use super::*;

    #[test]
    fn put_unsign_number_to_builder_success() {
        test_case(
            "PUSHINT 1
             NEWC
             PUSHINT 32
             STUXQ
             DROP",
        )
        .expect_item(create::builder([0, 0, 0, 1, 0x80]));

        let mut data = vec![0xff; 32];
        data.push(0x80);
        let mut builder = BuilderData::new();
        builder.append_raw(data.as_slice(), 256).unwrap();
        builder.append_raw(data.as_slice(), 256).unwrap();
        builder.append_raw(data.as_slice(), 256).unwrap();

        test_case(
            "PUSHINT 1
             PUSHINT -1
             PUSHINT -1
             PUSHINT -1
             NEWC
             STI 256
             STI 256
             STI 256
             PUSHINT 256
             STUXQ",
        )
        .expect_stack(
            Stack::new()
                .push(StackItem::int(IntegerData::one()))
                .push(StackItem::builder(builder))
                .push(int!(-1)),
        );
    }
    #[test]
    fn put_unsign_number_to_builder_type_error() {
        test_case(
            "PUSHSLICE x8_
             NEWC
             PUSHINT 30
             STUXQ",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT -1
             NEWC
             PUSHSLICE x8_
             STUXQ",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             NEWC
             PUSHINT 30
             STUXQ",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT -1
             NEWC
             NEWC
             STUXQ",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             ENDC
             NEWC
             PUSHINT 30
             STUXQ",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT -1
             NEWC
             NEWC
             ENDC
             STUXQ",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT -1
             NEWC
             ENDC
             PUSHINT 8
             STUXQ",
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }

    #[test]
    fn put_unsign_number_to_builder_underflow_error() {
        test_case(
            "PUSHINT 30
             NEWC
             STUXQ",
        )
        .expect_failure(ExceptionCode::StackUnderflow);

        test_case(
            "NEWC
             STUXQ",
        )
        .expect_failure(ExceptionCode::StackUnderflow);
    }

    #[test]
    fn put_unsign_number_to_builder_range_error() {
        test_case(
            "PUSHINT -1
             NEWC
             PUSHINT -1
             STUXQ",
        )
        .expect_failure(ExceptionCode::RangeCheckError);

        test_case(
            "PUSHINT -1
             NEWC
             PUSHINT 258
             STUXQ",
        )
        .expect_failure(ExceptionCode::RangeCheckError);
    }
}

mod stixrq {
    use super::*;

    #[test]
    fn put_sign_quiet_len_number_rev_to_builder_success() {
        test_case(
            "NEWC
             PUSHINT -1
             PUSHINT 30
             STIXRQ",
        )
        .expect_stack(Stack::new().push(create::builder([0xFF, 0xFF, 0xFF, 0xFE])).push(int!(0)));

        let mut data = vec![0xff; 32];
        data.push(0x80);
        let mut builder = BuilderData::new();
        builder.append_raw(data.as_slice(), 256).unwrap();
        builder.append_raw(data.as_slice(), 256).unwrap();
        builder.append_raw(data.as_slice(), 256).unwrap();

        test_case(
            "PUSHINT -1
             PUSHINT -1
             PUSHINT -1
             NEWC
             STI 256
             STI 256
             STI 256
             PUSHINT 1
             PUSHINT 256
             STIXRQ",
        )
        .expect_stack(
            Stack::new()
                .push(StackItem::builder(builder))
                .push(StackItem::int(IntegerData::one()))
                .push(int!(-1)),
        );
    }
    #[test]
    fn put_sign_number_to_builder_type_error() {
        test_case(
            "NEWC
             PUSHSLICE x8_
             PUSHINT 30
             STIXRQ",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             PUSHINT -1
             PUSHSLICE x8_
             STIXRQ",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             NEWC
             PUSHINT 30
             STIXRQ",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             PUSHINT -1
             NEWC
             STIXRQ",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             NEWC
             ENDC
             PUSHINT 30
             STIXRQ",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             PUSHINT -1
             NEWC
             ENDC
             STIXRQ",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             ENDC
             PUSHINT -1
             PUSHINT 8
             STIXRQ",
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }

    #[test]
    fn put_sign_number_to_builder_underflow_error() {
        test_case(
            "NEWC
             PUSHINT 30
             STIXRQ",
        )
        .expect_failure(ExceptionCode::StackUnderflow);

        test_case(
            "NEWC
             STIXRQ",
        )
        .expect_failure(ExceptionCode::StackUnderflow);
    }

    #[test]
    fn put_sign_number_to_builder_range_error() {
        test_case(
            "NEWC
             PUSHINT -1
             PUSHINT -1
             STIXRQ",
        )
        .expect_failure(ExceptionCode::RangeCheckError);

        test_case(
            "NEWC
             PUSHINT -1
             PUSHINT 258
             STIXRQ",
        )
        .expect_failure(ExceptionCode::RangeCheckError);
    }
}

mod stuxrq {
    use super::*;

    #[test]
    fn put_unsign_quiet_len_number_rev_to_builder_success() {
        test_case(
            "NEWC
             PUSHINT 1
             PUSHINT 32
             STUXRQ
             DROP",
        )
        .expect_item(create::builder([0, 0, 0, 1, 0x80]));

        let mut data = vec![0xff; 32];
        data.push(0x80);
        let mut builder = BuilderData::new();
        builder.append_raw(data.as_slice(), 256).unwrap();
        builder.append_raw(data.as_slice(), 256).unwrap();
        builder.append_raw(data.as_slice(), 256).unwrap();

        test_case(
            "PUSHINT -1
             PUSHINT -1
             PUSHINT -1
             NEWC
             STI 256
             STI 256
             STI 256
             PUSHINT 1
             PUSHINT 256
             STUXRQ",
        )
        .expect_stack(
            Stack::new()
                .push(StackItem::builder(builder))
                .push(StackItem::int(IntegerData::one()))
                .push(int!(-1)),
        );
    }
    #[test]
    fn put_unsign_number_to_builder_type_error() {
        test_case(
            "NEWC
             PUSHSLICE x8_
             PUSHINT 30
             STUXRQ",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             PUSHINT -1
             PUSHSLICE x8_
             STUXRQ",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             NEWC
             PUSHINT 30
             STUXRQ",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             PUSHINT -1
             NEWC
             STUXRQ",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             NEWC
             ENDC
             PUSHINT 30
             STUXRQ",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             PUSHINT -1
             NEWC
             ENDC
             STUXRQ",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             ENDC
             PUSHINT -1
             PUSHINT 8
             STUXRQ",
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }

    #[test]
    fn put_unsign_number_to_builder_underflow_error() {
        test_case(
            "NEWC
             PUSHINT 30
             STUXRQ",
        )
        .expect_failure(ExceptionCode::StackUnderflow);

        test_case(
            "NEWC
             STUXRQ",
        )
        .expect_failure(ExceptionCode::StackUnderflow);
    }

    #[test]
    fn put_unsign_number_to_builder_range_error() {
        test_case(
            "NEWC
             PUSHINT -1
             PUSHINT -1
             STUXRQ",
        )
        .expect_failure(ExceptionCode::RangeCheckError);

        test_case(
            "NEWC
             PUSHINT -1
             PUSHINT 258
             STUXRQ",
        )
        .expect_failure(ExceptionCode::RangeCheckError);
    }
}

mod stux {
    use super::*;

    #[test]
    fn put_unsign_len_number_to_builder_success() {
        test_case(
            "PUSHINT 1
             NEWC
             PUSHINT 32
             STUX",
        )
        .expect_item(create::builder([0, 0, 0, 1, 0x80]));
    }

    #[test]
    fn put_unsign_number_to_builder_celloverflow_error() {
        test_case(
            "PUSHINT 1
             PUSHINT -1
             PUSHINT -1
             PUSHINT -1
             NEWC
             STI 256
             STI 256
             STI 256
             PUSHINT 256
             STUX",
        )
        .expect_failure(ExceptionCode::CellOverflow);
    }

    #[test]
    fn put_unsign_number_to_builder_type_error() {
        test_case(
            "PUSHSLICE x8_
             NEWC
             PUSHINT 30
             STUX",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT -1
             NEWC
             PUSHSLICE x8_
             STUX",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             NEWC
             PUSHINT 30
             STUX",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT -1
             NEWC
             NEWC
             STUX",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             ENDC
             NEWC
             PUSHINT 30
             STUX",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT -1
             NEWC
             NEWC
             ENDC
             STUX",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT -1
             NEWC
             ENDC
             PUSHINT 8
             STUX",
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }

    #[test]
    fn put_unsign_number_to_builder_underflow_error() {
        test_case(
            "NEWC
             PUSHINT 30
             STUX",
        )
        .expect_failure(ExceptionCode::StackUnderflow);

        test_case(
            "NEWC
             STUX",
        )
        .expect_failure(ExceptionCode::StackUnderflow);
    }

    #[test]
    fn put_unsign_number_to_builder_range_error() {
        test_case(
            "PUSHINT -1
             NEWC
             PUSHINT -1
             STUX",
        )
        .expect_failure(ExceptionCode::RangeCheckError);

        test_case(
            "PUSHINT -1
             NEWC
             PUSHINT 258
             STUX",
        )
        .expect_failure(ExceptionCode::RangeCheckError);
    }
}

mod stu {
    use super::*;

    #[test]
    fn put_unsign_number_to_builder() {
        test_case(
            "PUSHINT 1
             NEWC
             STU 8",
        )
        .expect_item(create::builder([1, 0x80]));
    }

    #[test]
    fn put_unsign_number_to_builder_err_no_number() {
        test_case(
            "NEWC
             STU 8",
        )
        .expect_failure(ExceptionCode::StackUnderflow);

        test_case("STU 8").expect_failure(ExceptionCode::StackUnderflow);
    }

    #[test]
    fn put_unsign_number_to_builder_err_wrong_integer() {
        test_case(
            "
            PUSHINT 256
            NEWC
            STU 8
        ",
        )
        .expect_failure(ExceptionCode::RangeCheckError);

        test_case(
            "PUSHINT -1
             NEWC
             STU 258",
        )
        .expect_compilation_failure(CompileError::out_of_range(3, 14, "STU", "arg 0"));
    }

    #[test]
    fn put_unsign_number_to_builder_type_error() {
        test_case(
            "PUSHSLICE x8_
             NEWC
             STU 30",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             NEWC
             STU 30",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             ENDC
             NEWC
             STU 30",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT -1
             NEWC
             ENDC
             STU 30",
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }
}

mod stuxr {
    use super::*;

    #[test]
    fn put_unsign_len_number_rev_to_builder_with_a_remaining_element() {
        let mut builder = BuilderData::new();
        builder.append_raw([0x00, 0x80].to_vec().as_slice(), 6).unwrap();
        test_case(
            "PUSHINT 2
             NEWC
             PUSHINT 0
             PUSHINT 6
             STUXR",
        )
        .expect_stack(Stack::new().push(int!(2)).push(StackItem::builder(builder)));
    }

    #[test]
    fn put_unsign_len_number_rev_to_builder() {
        test_case(
            "NEWC
             PUSHINT 1
             PUSHINT 8
             STUXR",
        )
        .expect_item(create::builder([1, 0x80]));
    }

    #[test]
    fn put_unsign_len_number_to_builder_type_error() {
        test_case(
            "NEWC
             PUSHSLICE x8_
             PUSHINT 30
             STUXR",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             PUSHINT -1
             PUSHSLICE x8_
             STUXR",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             NEWC
             PUSHINT 30
             STUXR",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             PUSHINT -1
             NEWC
             STUXR",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             NEWC
             ENDC
             PUSHINT 30
             STUXR",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             PUSHINT -1
             NEWC
             ENDC
             STUXR",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             ENDC
             PUSHINT -1
             PUSHINT 8
             STUXR",
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }

    #[test]
    fn put_unsign_len_number_to_builder_underflow_error() {
        test_case(
            "NEWC
             PUSHINT 30
             STUXR",
        )
        .expect_failure(ExceptionCode::StackUnderflow);

        test_case(
            "NEWC
             STUXR",
        )
        .expect_failure(ExceptionCode::StackUnderflow);
    }

    #[test]
    fn put_unsign_len_number_to_builder_range_error() {
        test_case(
            "NEWC
             PUSHINT -1
             PUSHINT -1
             STUXR",
        )
        .expect_failure(ExceptionCode::RangeCheckError);

        test_case(
            "NEWC
             PUSHINT -1
             PUSHINT 258
             STUXR",
        )
        .expect_failure(ExceptionCode::RangeCheckError);
    }

    #[test]
    fn put_unsign_len_number_to_builder_cell_error() {
        test_case(
            "PUSHINT -1
             PUSHINT -1
             PUSHINT -1
             NEWC
             STI 256
             STI 256
             STI 256
             PUSHINT 1
             PUSHINT 256
             STUXR",
        )
        .expect_failure(ExceptionCode::CellOverflow);
    }
}

mod stref {
    use super::*;

    #[test]
    fn put_reference_to_builder() {
        let mut builder = BuilderData::new();
        builder.checked_append_reference(BuilderData::new().into_cell().unwrap()).unwrap();
        test_case(
            "NEWC
             ENDC
             NEWC
             STREF",
        )
        .expect_item(StackItem::builder(builder));
    }

    #[test]
    fn put_reference_with_data_to_builder() {
        let mut slice = SliceData::new_empty();
        slice.append_reference(SliceData::new(vec![1, 0x80])).unwrap();
        test_case(
            "PUSHINT 1
             NEWC
             STU 8
             ENDC
             NEWC
             STREF",
        )
        .expect_item(StackItem::builder(slice.into_builder().unwrap()));
    }

    #[test]
    fn put_reference_with_data_to_builder_with_data() {
        let mut slice = SliceData::new(vec![0xDE, 0xAD, 0xBE, 0xEF, 0x80]);
        slice.append_reference(SliceData::new(vec![1, 0x80])).unwrap();
        test_case(
            "PUSHINT 42
             PUSHINT 1
             NEWC
             STU 8
             ENDC
             PUSHINT 3735928559
             NEWC
             STU 32
             STREF",
        )
        .expect_stack(Stack::new().push(int!(42)).push_builder(slice.into_builder().unwrap()));
    }

    #[test]
    fn put_reference_to_builder_err_no_ref() {
        test_case(
            "NEWC
             STREF",
        )
        .expect_failure(ExceptionCode::StackUnderflow);
    }

    #[test]
    fn put_reference_to_builder_err_no_builder() {
        test_case(
            "PUSHINT 1
             NEWC
             ENDC
             STREF",
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }

    #[test]
    fn put_reference_to_builder_err_refs_overflow() {
        test_case(
            "NEWC
             ENDC
             NEWC
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
             STREF",
        )
        .expect_failure(ExceptionCode::CellOverflow);
    }

    #[test]
    fn put_reference_to_builder_param_error() {
        test_case(
            "PUSHINT 1
             NEWC
             STREF",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHSLICE x8_
             NEWC
             STREF",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             NEWC
             STREF",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             ENDC
             NEWC
             ENDC
             STREF",
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }
}

mod strefr {
    use super::*;

    #[test]
    fn put_reference_to_builder() {
        let mut builder = BuilderData::new();
        builder.checked_append_reference(BuilderData::new().into_cell().unwrap()).unwrap();
        test_case(
            "NEWC
             NEWC
             ENDC
             STREFR",
        )
        .expect_item(StackItem::builder(builder));
    }

    #[test]
    fn put_reference_with_data_to_builder() {
        let mut slice = SliceData::new_empty();
        slice.append_reference(SliceData::new(vec![1, 0x80])).unwrap();
        test_case(
            "NEWC
             PUSHINT 1
             NEWC
             STU 8
             ENDC
             STREFR",
        )
        .expect_item(StackItem::builder(slice.into_builder().unwrap()));
    }

    #[test]
    fn put_reference_with_data_to_builder_with_data() {
        let mut slice = SliceData::new(vec![0xDE, 0xAD, 0xBE, 0xEF, 0x80]);
        slice.append_reference(SliceData::new(vec![1, 0x80])).unwrap();
        test_case(
            "PUSHINT 42
             PUSHINT 3735928559
             NEWC
             STU 32
             PUSHINT 1
             NEWC
             STU 8
             ENDC
             STREFR",
        )
        .expect_stack(Stack::new().push(int!(42)).push_builder(slice.into_builder().unwrap()));
    }

    #[test]
    fn put_reference_to_builder_err_no_ref() {
        test_case(
            "NEWC
             STREFR",
        )
        .expect_failure(ExceptionCode::StackUnderflow);
    }

    #[test]
    fn put_reference_to_builder_err_no_builder() {
        test_case(
            "PUSHINT 1
             NEWC
             ENDC
             STREFR",
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }

    #[test]
    fn put_reference_to_builder_err_refs_overflow() {
        test_case(
            "NEWC
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
             NEWC
             ENDC
             STREFR",
        )
        .expect_failure(ExceptionCode::CellOverflow);
    }

    #[test]
    fn put_reference_to_builder_param_error() {
        test_case(
            "NEWC
             PUSHINT 1
             STREFR",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             PUSHSLICE x8_
             STREFR",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             NEWC
             STREFR",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             ENDC
             NEWC
             ENDC
             STREFR",
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }
}

mod stslice {
    use super::*;

    #[test]
    fn put_slice_to_builder() {
        test_case(
            "PUSHSLICE x8_
             NEWC
             STSLICE",
        )
        .expect_item(create::builder([0x80]));
    }

    #[test]
    fn put_slice_with_data_to_builder() {
        test_case(
            "PUSHSLICE xC_
             NEWC
             STSLICE",
        )
        .expect_item(create::builder([0xC0]));
    }

    #[test]
    fn put_slice_to_builder_no_arg_error() {
        test_case(
            "NEWC
             STSLICE",
        )
        .expect_failure(ExceptionCode::StackUnderflow);

        test_case(
            "PUSHSLICE x4_
             STSLICE",
        )
        .expect_failure(ExceptionCode::StackUnderflow);
    }

    #[test]
    fn put_slice_to_builder_param_error() {
        test_case(
            "PUSHINT 1
             NEWC
             STSLICE",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHSLICE x4_
             PUSHINT 1
             STSLICE",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             NEWC
             STSLICE",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHSLICE x4_
             PUSHSLICE x8_
             STSLICE",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             ENDC
             NEWC
             STSLICE",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHSLICE x4_
             NEWC
             ENDC
             STSLICE",
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }

    #[test]
    fn put_slice_to_builder_cellover_error() {
        test_case(
            "PUSHSLICE xFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF_
             PUSHSLICE xFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF_
             PUSHSLICE xFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF_
             PUSHSLICE xFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF_
             NEWC
             STSLICE
             STSLICE
             STSLICE
             STSLICE",
        )
        .expect_failure(ExceptionCode::CellOverflow);
    }
}

mod stslicer {
    use super::*;

    #[test]
    fn put_slice_to_builder() {
        test_case(
            "NEWC
             PUSHSLICE x8_
             STSLICER",
        )
        .expect_item(create::builder([0x80]));
    }

    #[test]
    fn put_slice_with_data_to_builder() {
        test_case(
            "NEWC
             PUSHSLICE xC_
             STSLICER",
        )
        .expect_item(create::builder([0xC0]));
    }

    #[test]
    fn put_slice_to_builder_no_arg_error() {
        test_case(
            "NEWC
             STSLICER",
        )
        .expect_failure(ExceptionCode::StackUnderflow);

        test_case(
            "PUSHSLICE x4_
             STSLICER",
        )
        .expect_failure(ExceptionCode::StackUnderflow);
    }

    #[test]
    fn put_slice_to_builder_param_error() {
        test_case(
            "NEWC
             PUSHINT 1
             STSLICER",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT 1
             PUSHSLICE x4_
             STSLICER",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             NEWC
             STSLICER",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHSLICE x8_
             PUSHSLICE x4_
             STSLICER",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             NEWC
             ENDC
             STSLICER",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             ENDC
             PUSHSLICE x4_
             STSLICER",
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }

    #[test]
    fn put_slice_to_builder_cellover_error() {
        test_case(
            "PUSHSLICE xFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF_
             PUSHSLICE xFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF_
             PUSHSLICE xFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF_
             NEWC
             STSLICE
             STSLICE
             STSLICE
             PUSHSLICE xFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF_
             STSLICER",
        )
        .expect_failure(ExceptionCode::CellOverflow);
    }
}

mod stsliceq {
    use super::*;

    #[test]
    fn put_slice_to_builder() {
        test_case(
            "PUSHSLICE x8_
             NEWC
             STSLICEQ
             DROP",
        )
        .expect_item(create::builder([0x80]));

        let mut data = vec![0xff; 32];
        data.push(0x80);
        let mut builder = BuilderData::new();
        builder.append_raw(data.as_slice(), 255).unwrap();
        builder.append_raw(data.as_slice(), 256).unwrap();
        builder.append_raw(data.as_slice(), 256).unwrap();
        builder.append_raw(data.as_slice(), 256).unwrap();
        test_case(
            "PUSHSLICE xC_
             PUSHINT -1
             PUSHINT -1
             PUSHINT -1
             PUSHINT -1
             NEWC
             STI 255
             STI 256
             STI 256
             STI 256
             STSLICEQ",
        )
        .expect_stack(
            Stack::new()
                .push(StackItem::Slice(SliceData::new(vec![0xC0])))
                .push_builder(builder)
                .push(int!(-1)),
        );
    }

    #[test]
    fn put_slice_with_data_to_builder() {
        test_case(
            "PUSHSLICE xC_
             NEWC
             STSLICEQ
             DROP",
        )
        .expect_item(create::builder([0xC0]));
    }

    #[test]
    fn put_slice_to_builder_no_arg_error() {
        test_case(
            "NEWC
             STSLICEQ",
        )
        .expect_failure(ExceptionCode::StackUnderflow);

        test_case(
            "PUSHSLICE x4_
             STSLICEQ",
        )
        .expect_failure(ExceptionCode::StackUnderflow);
    }

    #[test]
    fn put_slice_to_builder_param_error() {
        test_case(
            "PUSHINT 1
             NEWC
             STSLICEQ",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHSLICE x4_
             PUSHINT 1
             STSLICEQ",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             NEWC
             STSLICEQ",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHSLICE x4_
             PUSHSLICE x8_
             STSLICEQ",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             ENDC
             NEWC
             STSLICEQ",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHSLICE x4_
             NEWC
             ENDC
             STSLICEQ",
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }
}

mod stslicerq {
    use super::*;

    #[test]
    fn put_slice_to_builder() {
        test_case(
            "NEWC
             PUSHSLICE x8_
             STSLICERQ
             DROP",
        )
        .expect_item(create::builder([0x80]));

        let mut data = vec![0xff; 32];
        data.push(0x80);
        let mut builder = BuilderData::new();
        builder.append_raw(data.as_slice(), 255).unwrap();
        builder.append_raw(data.as_slice(), 256).unwrap();
        builder.append_raw(data.as_slice(), 256).unwrap();
        builder.append_raw(data.as_slice(), 256).unwrap();
        test_case(
            "PUSHINT -1
             PUSHINT -1
             PUSHINT -1
             PUSHINT -1
             NEWC
             STI 255
             STI 256
             STI 256
             STI 256
             PUSHSLICE xC_
             STSLICERQ",
        )
        .expect_stack(
            Stack::new()
                .push_builder(builder)
                .push(StackItem::Slice(SliceData::new(vec![0xC0])))
                .push(int!(-1)),
        );
    }

    #[test]
    fn put_slice_with_data_to_builder() {
        test_case(
            "NEWC
             PUSHSLICE xC_
             STSLICERQ
             DROP",
        )
        .expect_item(create::builder([0xC0]));
    }

    #[test]
    fn put_slice_to_builder_no_arg_error() {
        test_case(
            "NEWC
             STSLICERQ",
        )
        .expect_failure(ExceptionCode::StackUnderflow);

        test_case(
            "PUSHSLICE x4_
             STSLICERQ",
        )
        .expect_failure(ExceptionCode::StackUnderflow);
    }

    #[test]
    fn put_slice_to_builder_param_error() {
        test_case(
            "NEWC
             PUSHINT 1
             STSLICERQ",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT 1
             PUSHSLICE x4_
             STSLICERQ",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             NEWC
             STSLICERQ",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHSLICE x4_
             PUSHSLICE x8_
             STSLICERQ",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             NEWC
             ENDC
             STSLICERQ",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             ENDC
             PUSHSLICE x4_
             STSLICERQ",
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }
}

mod stbr {
    use super::*;

    #[test]
    fn put_builder_to_builder() {
        test_case(
            "NEWC
             NEWC
             STBR",
        )
        .expect_item(create::builder([0x80]));
    }

    #[test]
    fn put_builder_with_data_to_builder() {
        test_case(
            "NEWC
             PUSHINT 1
             NEWC
             STU 8
             STBR",
        )
        .expect_item(create::builder([1, 0x80]));

        let mut data = vec![0xff; 32];
        data.push(0x80);
        test_case(
            "NEWC
             PUSHPOW2 255
             PUSHPOW2 255
             ADDCONST -1
             ADD
             NEWC
             STU 256
             STBR",
        )
        .expect_stack(Stack::new().push(create::builder(data)));
    }

    #[test]
    fn put_builder_to_builder_err_no_builder() {
        test_case(
            "NEWC
             STBR",
        )
        .expect_failure(ExceptionCode::StackUnderflow);
    }
    #[test]
    fn put_builder_to_builder_type_err() {
        test_case(
            "NEWC
             PUSHINT 1
             STBR",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT 1
             NEWC
             STBR",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             PUSHSLICE x8_
             STBR",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHSLICE x8_
             NEWC
             STBR",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             NEWC
             ENDC
             STBR",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             ENDC
             NEWC
             STBR",
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }
    #[test]
    fn put_builder_to_builder_over() {
        test_case(
            "PUSHPOW2 255
             PUSHPOW2 255
             PUSHPOW2 255
             NEWC
             STU 256
             STU 256
             STU 256
             PUSHPOW2 255
             PUSHPOW2 255
             PUSHPOW2 255
             NEWC
             STU 256
             STU 256
             STU 256
             STBR",
        )
        .expect_failure(ExceptionCode::CellOverflow);
    }
}

mod strefq {
    use super::*;

    #[test]
    fn put_reference_to_builder() {
        let mut builder = BuilderData::new();
        builder.checked_append_reference(BuilderData::new().into_cell().unwrap()).unwrap();
        test_case(
            "PUSHINT 42
             NEWC
             ENDC
             NEWC
             STREFQ",
        )
        .expect_stack(Stack::new().push(int!(42)).push_builder(builder).push(int!(0)));
    }

    #[test]
    fn put_reference_with_data_to_builder() {
        let mut slice = SliceData::new_empty();
        slice.append_reference(SliceData::new(vec![1, 0x80])).unwrap();
        test_case(
            "PUSHINT 42
             PUSHINT 1
             NEWC
             STU 8
             ENDC
             NEWC
             STREFQ",
        )
        .expect_stack(
            Stack::new().push(int!(42)).push_builder(slice.into_builder().unwrap()).push(int!(0)),
        );
    }

    #[test]
    fn put_reference_with_data_to_builder_with_data() {
        let mut slice = SliceData::new(vec![0xDE, 0xAD, 0xBE, 0xEF, 0x80]);
        slice.append_reference(SliceData::new(vec![1, 0x80])).unwrap();
        test_case(
            "PUSHINT 42
             PUSHINT 1
             NEWC
             STU 8
             ENDC
             PUSHINT 3735928559
             NEWC
             STU 32
             STREFQ",
        )
        .expect_stack(
            Stack::new().push(int!(42)).push_builder(slice.into_builder().unwrap()).push(int!(0)),
        );
    }

    #[test]
    fn neg_missed_all_args() {
        test_case("STREFQ").expect_failure(ExceptionCode::StackUnderflow);
    }

    #[test]
    fn neg_missed_part_of_args() {
        test_case(
            "NEWC
             ENDC
             STREFQ",
        )
        .expect_failure(ExceptionCode::StackUnderflow);
    }

    #[test]
    fn neg_wrong_type_of_arg_1() {
        test_case(
            "PUSHINT 42
             NEWC
             STREFQ",
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }

    #[test]
    fn neg_wrong_type_of_arg_2() {
        test_case(
            "PUSHINT 42
             NEWC
             ENDC
             NEWC
             ENDC
             STREFQ",
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }

    #[test]
    fn neg_quiet_refs_num_exceed() {
        let mut builder = BuilderData::new();
        builder.checked_append_reference(BuilderData::new().into_cell().unwrap()).unwrap();
        builder.checked_append_reference(BuilderData::new().into_cell().unwrap()).unwrap();
        builder.checked_append_reference(BuilderData::new().into_cell().unwrap()).unwrap();
        builder.checked_append_reference(BuilderData::new().into_cell().unwrap()).unwrap();
        test_case(
            "PUSHINT 42
             NEWC
             ENDC
             NEWC
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
             STREFQ",
        )
        .expect_stack(
            Stack::new()
                .push(int!(42))
                .push(create::cell([0x80]))
                .push_builder(builder)
                .push(int!(-1)),
        );
    }

    #[test]
    fn put_reference_to_builder_param_error() {
        test_case(
            "PUSHINT 1
             NEWC
             STREFQ",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHSLICE x8_
             NEWC
             STREFQ",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             NEWC
             STREFQ",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             ENDC
             NEWC
             ENDC
             STREFQ",
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }
}

mod strefrq {
    use super::*;

    #[test]
    fn put_reference_to_builder() {
        let mut builder = BuilderData::new();
        builder.checked_append_reference(BuilderData::new().into_cell().unwrap()).unwrap();
        test_case(
            "PUSHINT 42
             NEWC
             NEWC
             ENDC
             STREFRQ",
        )
        .expect_stack(Stack::new().push(int!(42)).push_builder(builder).push(int!(0)));
    }

    #[test]
    fn put_reference_with_data_to_builder() {
        let mut slice = SliceData::new_empty();
        slice.append_reference(SliceData::new(vec![1, 0x80])).unwrap();
        test_case(
            "PUSHINT 42
             NEWC
             PUSHINT 1
             NEWC
             STU 8
             ENDC
             STREFRQ",
        )
        .expect_stack(
            Stack::new().push(int!(42)).push_builder(slice.into_builder().unwrap()).push(int!(0)),
        );
    }

    #[test]
    fn put_reference_with_data_to_builder_with_data() {
        let mut slice = SliceData::new(vec![1, 0x80]);
        slice.append_reference(SliceData::new(vec![0xDE, 0xAD, 0xBE, 0xEF, 0x80])).unwrap();
        test_case(
            "PUSHINT 42
             PUSHINT 1
             NEWC
             STU 8
             PUSHINT 3735928559
             NEWC
             STU 32
             ENDC
             STREFRQ",
        )
        .expect_stack(
            Stack::new().push(int!(42)).push_builder(slice.into_builder().unwrap()).push(int!(0)),
        );
    }

    #[test]
    fn neg_missed_all_args() {
        test_case("STREFRQ").expect_failure(ExceptionCode::StackUnderflow);
    }

    #[test]
    fn neg_missed_part_of_args() {
        test_case(
            "NEWC
             ENDC
             STREFRQ",
        )
        .expect_failure(ExceptionCode::StackUnderflow);
    }

    #[test]
    fn neg_wrong_type_of_arg_1() {
        test_case(
            "PUSHINT 42
             NEWC
             ENDC
             STREFRQ",
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }

    #[test]
    fn neg_wrong_type_of_arg_2() {
        test_case(
            "PUSHINT 42
             NEWC
             NEWC
             STREFRQ",
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }

    #[test]
    fn neg_quiet_refs_num_exceed() {
        let mut builder = BuilderData::new();
        builder.checked_append_reference(BuilderData::new().into_cell().unwrap()).unwrap();
        builder.checked_append_reference(BuilderData::new().into_cell().unwrap()).unwrap();
        builder.checked_append_reference(BuilderData::new().into_cell().unwrap()).unwrap();
        builder.checked_append_reference(BuilderData::new().into_cell().unwrap()).unwrap();
        test_case(
            "PUSHINT 42
             NEWC
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
             NEWC
             ENDC
             STREFRQ",
        )
        .expect_stack(
            Stack::new()
                .push(int!(42))
                .push_builder(builder)
                .push(create::cell([0x80]))
                .push(int!(-1)),
        );
    }

    #[test]
    fn put_reference_to_builder_param_error() {
        test_case(
            "PUSHINT 1
             NEWC
             STREFRQ",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHSLICE x8_
             NEWC
             STREFRQ",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             NEWC
             STREFRQ",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             ENDC
             NEWC
             ENDC
             STREFRQ",
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }
}

// STREFCONST, equivalent to PUSHREF; STREFR.
mod strefconst {
    use super::*;

    #[test]
    fn basic_scenario() {
        let slice = SliceData::new(PUSHINT1.to_vec()).into_cell().unwrap();
        let mut builder = BuilderData::new();
        builder.checked_append_reference(slice.clone()).unwrap();

        test_case_with_refs(
            "NEWC
             STREFCONST",
            vec![slice],
        )
        .expect_item(StackItem::builder(builder));
    }

    #[test]
    fn neg_missed_all_args() {
        test_case("STREFCONST").expect_failure(ExceptionCode::InvalidOpcode);
    }

    #[test]
    fn neg_missed_part_of_args_1() {
        let slice = SliceData::new(PUSHINT1.to_vec()).into_cell().unwrap();
        test_case_with_refs("STREFCONST", vec![slice])
            .expect_failure(ExceptionCode::StackUnderflow);
    }

    #[test]
    fn neg_missed_part_of_args_2() {
        test_case(
            "NEWC
             STREFCONST",
        )
        .expect_failure(ExceptionCode::InvalidOpcode);
    }

    #[test]
    fn neg_wrong_type_of_arg_1() {
        let slice = SliceData::new(PUSHINT1.to_vec()).into_cell().unwrap();
        test_case_with_refs(
            "NEWC
             ENDC
             STREFCONST",
            vec![slice.clone()],
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case_with_refs(
            "PUSHINT 1
             STREFCONST",
            vec![slice.clone()],
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case_with_refs(
            "PUSHSLICE xC_
             STREFCONST",
            vec![slice],
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }
}

// STREF2CONST, equivalent to STREFCONST; STREFCONST.
mod stref2const {
    use super::*;

    #[test]
    fn basic_scenario() {
        let slice1 = SliceData::new(PUSHINT1.to_vec()).into_cell().unwrap();
        let slice2 = SliceData::new(PUSHINT2.to_vec()).into_cell().unwrap();
        let mut builder = BuilderData::new();
        builder.checked_append_reference(slice1.clone()).unwrap();
        builder.checked_append_reference(slice2.clone()).unwrap();

        test_case_with_refs(
            "NEWC
             STREF2CONST",
            vec![slice1, slice2],
        )
        .expect_item(StackItem::builder(builder));
    }

    #[test]
    fn neg_missed_part_of_args_1() {
        let slice1 = SliceData::new(PUSHINT1.to_vec()).into_cell().unwrap();
        let slice2 = SliceData::new(PUSHINT2.to_vec()).into_cell().unwrap();

        test_case_with_refs("STREF2CONST", vec![slice1, slice2])
            .expect_failure(ExceptionCode::StackUnderflow);
    }

    #[test]
    fn neg_missed_part_of_args_2() {
        let slice1 = SliceData::new(PUSHINT1.to_vec()).into_cell().unwrap();

        test_case_with_refs(
            "NEWC
             STREF2CONST",
            vec![slice1],
        )
        .expect_failure(ExceptionCode::InvalidOpcode);
    }

    #[test]
    fn neg_exceed_refs() {
        let slice1 = SliceData::new(PUSHINT1.to_vec()).into_cell().unwrap();
        let slice2 = SliceData::new(PUSHINT2.to_vec()).into_cell().unwrap();
        let slice3 = SliceData::new(PUSHINT3.to_vec()).into_cell().unwrap();
        let mut builder = BuilderData::new();
        builder.checked_append_reference(slice1.clone()).unwrap();
        builder.checked_append_reference(slice2.clone()).unwrap();

        test_case_with_refs(
            "NEWC
             STREF2CONST",
            vec![slice1, slice2, slice3],
        )
        .expect_stack(Stack::new().push_builder(builder).push(int!(3)));
    }

    #[test]
    fn neg_wrong_type_of_arg_1() {
        let slice1 = SliceData::new(PUSHINT1.to_vec()).into_cell().unwrap();
        let slice2 = SliceData::new(PUSHINT2.to_vec()).into_cell().unwrap();

        test_case_with_refs(
            "NEWC
             ENDC
             STREF2CONST",
            vec![slice1, slice2],
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }
}

mod stref3const {
    use super::*;

    #[test]
    fn basic_scenario() {
        let slice1 = SliceData::new(PUSHINT1.to_vec()).into_cell().unwrap();
        let slice2 = SliceData::new(PUSHINT2.to_vec()).into_cell().unwrap();
        let slice3 = SliceData::new(PUSHINT3.to_vec()).into_cell().unwrap();
        let mut builder = BuilderData::new();
        builder.checked_append_reference(slice1.clone()).unwrap();
        builder.checked_append_reference(slice2.clone()).unwrap();
        builder.checked_append_reference(slice3.clone()).unwrap();

        test_case_with_refs(
            "NEWC
             STREF3CONST",
            vec![slice1, slice2, slice3],
        )
        .expect_item(StackItem::builder(builder));
    }

    #[test]
    fn neg_missed_part_of_args_1() {
        let slice1 = SliceData::new(PUSHINT1.to_vec()).into_cell().unwrap();
        let slice2 = SliceData::new(PUSHINT2.to_vec()).into_cell().unwrap();
        let slice3 = SliceData::new(PUSHINT3.to_vec()).into_cell().unwrap();

        test_case_with_refs("STREF3CONST", vec![slice1, slice2, slice3])
            .expect_failure(ExceptionCode::StackUnderflow);
    }

    #[test]
    fn neg_missed_part_of_args_2() {
        let slice1 = SliceData::new(PUSHINT1.to_vec()).into_cell().unwrap();
        let slice2 = SliceData::new(PUSHINT2.to_vec()).into_cell().unwrap();

        test_case_with_refs(
            "NEWC
             STREF3CONST",
            vec![slice1, slice2],
        )
        .expect_failure(ExceptionCode::InvalidOpcode);
    }

    #[test]
    fn neg_missed_part_of_args_2_2() {
        let slice1 = SliceData::new(PUSHINT1.to_vec()).into_cell().unwrap();
        let slice2 = SliceData::new(PUSHINT2.to_vec()).into_cell().unwrap();

        test_case_with_refs(
            "NEWC
             STREF3CONST",
            vec![slice2, slice1],
        )
        .expect_failure(ExceptionCode::InvalidOpcode);
    }

    #[test]
    fn neg_missed_part_of_args_3() {
        let slice1 = SliceData::new(PUSHINT1.to_vec()).into_cell().unwrap();
        //~ let slice2 = SliceData::new(PUSHINT2.to_vec());

        test_case_with_refs(
            "NEWC
             STREF3CONST",
            vec![slice1],
        )
        .expect_failure(ExceptionCode::InvalidOpcode);
    }

    #[test]
    fn neg_wrong_type_of_arg_1() {
        let slice1 = SliceData::new(PUSHINT1.to_vec()).into_cell().unwrap();
        let slice2 = SliceData::new(PUSHINT2.to_vec()).into_cell().unwrap();
        let slice3 = SliceData::new(PUSHINT3.to_vec()).into_cell().unwrap();

        test_case_with_refs(
            "NEWC
             ENDC
             STREF3CONST",
            vec![slice1, slice2, slice3],
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }
}

mod stile4 {
    use super::*;

    #[test]
    fn put_sign_32b_number_to_builder() {
        test_case(
            "PUSHINT -2
             NEWC
             STILE4",
        )
        .expect_item(create::builder([0xFE, 0xFF, 0xFF, 0xFF, 0x80]));
    }

    #[test]
    fn put_sign_32b_number_to_builder_range_check() {
        test_case(
            "PUSHINT 2147483647
             NEWC
             STILE4",
        )
        .expect_item(create::builder([0xFF, 0xFF, 0xFF, 0x7F, 0x80]));

        test_case(
            "PUSHINT -2147483648
             NEWC
             STILE4",
        )
        .expect_item(create::builder([0x00, 0x00, 0x00, 0x80, 0x80]));

        test_case(
            "PUSHINT 2147483648
             NEWC
             STILE4",
        )
        .expect_failure(ExceptionCode::RangeCheckError);

        test_case(
            "PUSHINT -2147483649
             NEWC
             STILE4",
        )
        .expect_failure(ExceptionCode::RangeCheckError);
    }

    #[test]
    fn put_sign_32b_number_to_builder_param_err() {
        test_case(
            "PUSHSLICE xC_
             NEWC
             STILE4",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             NEWC
             STILE4",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             ENDC
             NEWC
             STILE4",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT -2
             PUSHINT 1
             STILE4",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT -2
             PUSHSLICE xC_
             STILE4",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT -2
             NEWC
             ENDC
             STILE4",
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }

    #[test]
    fn put_sign_32b_number_to_builder_no_arg_err() {
        test_case(
            "NEWC
             STILE4",
        )
        .expect_failure(ExceptionCode::StackUnderflow);

        test_case(
            "PUSHSLICE xC_
             STILE4",
        )
        .expect_failure(ExceptionCode::StackUnderflow);
    }

    #[test]
    fn put_sign_32b_number_to_builder_no_place_err() {
        test_case(
            "PUSHINT -1
             DUP
             DUP
             DUP
             DUP
             NEWC
             STI 255
             STI 255
             STI 255
             STI 255
             STILE4",
        )
        .expect_failure(ExceptionCode::CellOverflow);
    }
}

mod stule4 {
    use super::*;

    #[test]
    fn put_unsign_32b_number_to_builder() {
        test_case(
            "PUSHINT 1
             NEWC
             STULE4",
        )
        .expect_item(create::builder([1, 0, 0, 0, 0x80]));
    }

    #[test]
    fn put_unsign_32b_max_number_to_builder() {
        let result_vector = vec![0xFF, 0xFF, 0xFF, 0xFF, 0x80];
        test_case(
            "PUSHINT 4294967295
             NEWC
             STULE4",
        )
        .expect_item(create::builder(result_vector));

        test_case(
            "PUSHINT 4294967296
             NEWC
             STULE4",
        )
        .expect_failure(ExceptionCode::RangeCheckError);

        test_case(
            "PUSHINT -1
             NEWC
             STULE4",
        )
        .expect_failure(ExceptionCode::RangeCheckError);
    }

    #[test]
    fn put_unsign_32b_min_number_to_builder() {
        test_case(
            "PUSHINT 0
             NEWC
             STULE4",
        )
        .expect_item(create::builder([0, 0, 0, 0, 0x80]));
    }

    #[test]
    fn put_unsign_32b_number_to_builder_err_no_number() {
        test_case(
            "NEWC
             STULE4",
        )
        .expect_failure(ExceptionCode::StackUnderflow);
    }

    #[test]
    fn put_unsign_32b_number_to_builder_err_no_builder() {
        test_case(
            "NEWC
             PUSHINT 1
             STULE4",
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }

    #[test]
    fn put_unsign_32b_number_to_builder_param_err() {
        test_case(
            "PUSHSLICE xC_
             NEWC
             STULE4",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             NEWC
             STULE4",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             ENDC
             NEWC
             STULE4",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT -2
             PUSHINT 1
             STULE4",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT -2
             PUSHSLICE xC_
             STULE4",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT -2
             NEWC
             ENDC
             STULE4",
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }

    #[test]
    fn put_unsign_32b_number_to_builder_no_place_err() {
        test_case(
            "PUSHINT 1
             PUSHINT -1
             DUP
             DUP
             DUP
             NEWC
             STI 255
             STI 255
             STI 255
             STI 255
             STULE4",
        )
        .expect_failure(ExceptionCode::CellOverflow);
    }
}

mod stile8 {
    use super::*;

    #[test]
    fn put_sign_64b_number_to_builder() {
        test_case(
            "PUSHINT -2
             NEWC
             STILE8",
        )
        .expect_item(create::builder([0xFE, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x80]));
    }

    #[test]
    fn put_sign_64b_number_to_builder_range_check() {
        test_case(
            "PUSHINT 9223372036854775807
             NEWC
             STILE8",
        )
        .expect_item(create::builder([0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x7F, 0x80]));

        test_case(
            "PUSHINT -9223372036854775808
             NEWC
             STILE8",
        )
        .expect_item(create::builder([0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80, 0x80]));

        test_case(
            "PUSHINT 9223372036854775808
             NEWC
             STILE8",
        )
        .expect_failure(ExceptionCode::RangeCheckError);

        test_case(
            "PUSHINT -9223372036854775809
             NEWC
             STILE8",
        )
        .expect_failure(ExceptionCode::RangeCheckError);
    }

    #[test]
    fn put_sign_64b_number_to_builder_param_err() {
        test_case(
            "PUSHSLICE xC_
             NEWC
             STILE8",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             NEWC
             STILE8",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             ENDC
             NEWC
             STILE8",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT -2
             PUSHINT 1
             STILE8",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT -2
             PUSHSLICE xC_
             STILE8",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHINT -2
             NEWC
             ENDC
             STILE8",
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }

    #[test]
    fn put_sign_64b_number_to_builder_no_arg_err() {
        test_case(
            "NEWC
             STILE8",
        )
        .expect_failure(ExceptionCode::StackUnderflow);

        test_case(
            "PUSHSLICE xC_
             STILE8",
        )
        .expect_failure(ExceptionCode::StackUnderflow);
    }

    #[test]
    fn put_sign_64b_number_to_builder_no_place_err() {
        test_case(
            "PUSHINT -1
             DUP
             DUP
             DUP
             DUP
             NEWC
             STI 255
             STI 255
             STI 255
             STI 255
             STILE8",
        )
        .expect_failure(ExceptionCode::CellOverflow);
    }
}

mod stule8 {
    use super::*;

    #[test]
    fn put_unsign_64b_number_to_builder() {
        test_case(
            "PUSHINT 1
             NEWC
             STULE8",
        )
        .expect_item(create::builder([1, 0, 0, 0, 0, 0, 0, 0, 0x80]));
    }

    #[test]
    fn put_unsign_64b_max_number_to_builder() {
        let result_vector = vec![0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x80];
        test_case(
            "PUSHINT 18446744073709551615
             NEWC
             STULE8",
        )
        .expect_item(create::builder(result_vector));
    }

    #[test]
    fn put_unsign_64b_min_number_to_builder() {
        test_case(
            "PUSHINT 0
             NEWC
             STULE8",
        )
        .expect_item(create::builder([0, 0, 0, 0, 0, 0, 0, 0, 0x80]));
    }

    #[test]
    fn put_unsign_64b_number_to_builder_err_no_number() {
        test_case(
            "NEWC
             STULE8",
        )
        .expect_failure(ExceptionCode::StackUnderflow);
    }

    #[test]
    fn put_unsign_64b_number_to_builder_err_no_builder() {
        test_case(
            "NEWC
             PUSHINT 1
             STULE8",
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }

    #[test]
    fn put_unsign_64b_number_to_builder_err_out_left_bound() {
        test_case(
            "PUSHINT -1
             NEWC
             STULE8",
        )
        .expect_failure(ExceptionCode::RangeCheckError);
    }

    #[test]
    fn put_unsign_64b_number_to_builder_err_out_left_right() {
        test_case(
            "PUSHINT 18446744073709551616
             NEWC
             STULE8",
        )
        .expect_failure(ExceptionCode::RangeCheckError);
    }

    #[test]
    fn put_unsign_32b_number_to_builder_no_place_err() {
        test_case(
            "PUSHINT 1
             PUSHINT -1
             DUP
             DUP
             DUP
             NEWC
             STI 255
             STI 255
             STI 255
             STI 255
             STULE8",
        )
        .expect_failure(ExceptionCode::CellOverflow);
    }
}

mod bbits {
    use super::*;

    #[test]
    fn get_count_stored_bits_in_builder() {
        test_case(
            "NEWC
             BBITS",
        )
        .expect_item(int!(0));
    }

    #[test]
    fn get_count_stored_bits_in_builder_with_data() {
        test_case(
            "PUSHINT 1
             NEWC
             STU 8
             BBITS",
        )
        .expect_item(int!(8));

        test_case(
            "PUSHINT 1
             DUP
             DUP
             DUP
             NEWC
             STU 255
             STU 256
             STU 256
             STU 256
             BBITS",
        )
        .expect_item(int!(1023));
    }

    #[test]
    fn get_count_stored_bits_in_builder_err_no_builder() {
        test_case(
            "PUSHINT 1
             BBITS",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHSLICE xC_
             BBITS",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             ENDC
             BBITS",
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }
}

mod brefs {
    use super::*;

    #[test]
    fn get_count_stored_refs_in_builder() {
        test_case(
            "NEWC
             BREFS",
        )
        .expect_item(int!(0));
    }

    #[test]
    fn get_count_stored_refs_in_builder_with_data() {
        let mut slice = SliceData::new_empty();
        slice.append_reference(SliceData::new_empty()).unwrap();
        test_case(
            "NEWC
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
             BREFS",
        )
        .expect_item(int!(4));
    }

    #[test]
    fn get_count_stored_refs_in_builder_type_err() {
        test_case(
            "PUSHINT 1
             BREFS",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHSLICE xC_
             BREFS",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             ENDC
             BREFS",
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }
}

mod bbitrefs {
    use super::*;

    #[test]
    fn get_count_stored_refs_and_bits_in_builder() {
        test_case(
            "NEWC
             BBITREFS",
        )
        .expect_stack(Stack::new().push(int!(0)).push(int!(0)));
    }

    #[test]
    fn get_count_stored_refs_and_bits_in_builder_with_data() {
        let mut slice = SliceData::new_empty();
        slice.append_reference(SliceData::new_empty()).unwrap();
        test_case(
            "NEWC
             ENDC
             NEWC
             STREF
             PUSHINT 1
             SWAP
             STU 8
             BBITREFS",
        )
        .expect_stack(Stack::new().push(int!(8)).push(int!(1)));
    }

    #[test]
    fn get_count_stored_refs_and_bits_in_builder_type_err() {
        test_case(
            "PUSHINT 1
             BBITREFS",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHSLICE xC_
             BBITREFS",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             ENDC
             BBITREFS",
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }
}

mod brembits {
    use super::*;

    #[test]
    fn get_count_free_bits_in_builder() {
        test_case(
            "NEWC
             BREMBITS",
        )
        .expect_item(int!(1023));
    }

    #[test]
    fn get_count_free_bits_in_builder_with_data() {
        test_case(
            "PUSHINT 1
             NEWC
             STU 8
             BREMBITS",
        )
        .expect_item(int!(1015));

        test_case(
            "PUSHINT 1
             PUSHINT 1
             PUSHINT 1
             PUSHINT 1
             NEWC
             STU 256
             STU 256
             STU 256
             STU 255
             BREMBITS",
        )
        .expect_item(int!(0));
    }

    #[test]
    fn get_count_free_bits_in_builder_type_err() {
        test_case(
            "PUSHINT 1
             BREMBITS",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHSLICE xC_
             BREMBITS",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             ENDC
             BREMBITS",
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }
}

mod bremrefs {
    use super::*;

    #[test]
    fn get_count_free_refs_in_builder() {
        test_case(
            "NEWC
             BREMREFS",
        )
        .expect_item(int!(4));
    }

    #[test]
    fn get_count_free_refs_in_builder_with_data() {
        let mut slice = SliceData::new_empty();
        slice.append_reference(SliceData::new_empty()).unwrap();
        test_case(
            "NEWC
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
             BREMREFS",
        )
        .expect_item(int!(0));
    }

    #[test]
    fn get_count_free_refs_in_builder_type_err() {
        test_case(
            "PUSHINT 1
             BREMREFS",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHSLICE xC_
             BREMREFS",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             ENDC
             BREMREFS",
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }
}

mod brembitrefs {
    use super::*;

    #[test]
    fn get_count_free_refs_and_bits_in_builder() {
        test_case(
            "NEWC
             BREMBITREFS",
        )
        .expect_stack(Stack::new().push(int!(1023)).push(int!(4)));
    }

    #[test]
    fn get_count_free_refs_and_bits_in_builder_with_data() {
        let mut slice = SliceData::new_empty();
        slice.append_reference(SliceData::new_empty()).unwrap();
        test_case(
            "NEWC
             ENDC
             NEWC
             STREF
             PUSHINT 1
             SWAP
             STU 8
             BREMBITREFS",
        )
        .expect_stack(Stack::new().push(int!(1015)).push(int!(3)));
    }

    #[test]
    fn get_count_free_refs_and_bits_in_builder_type_err() {
        test_case(
            "PUSHINT 1
             BREMBITREFS",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "PUSHSLICE xC_
             BREMBITREFS",
        )
        .expect_failure(ExceptionCode::TypeCheckError);

        test_case(
            "NEWC
             ENDC
             BREMBITREFS",
        )
        .expect_failure(ExceptionCode::TypeCheckError);
    }
}

mod bchkbits {
    use super::*;

    #[test]
    fn check_can_bits_stored_in_builder() {
        test_case(
            "
            ONE
            NEWC
            PUSHINT 1023
            BCHKBITS
        ",
        )
        .expect_int_stack(&[1]);
    }

    #[test]
    fn check_can_bits_stored_in_builder_with_data() {
        test_case(
            "
            ONE
            PUSHINT 1
            NEWC
            STU 8
            PUSHINT 1015
            BCHKBITS
        ",
        )
        .expect_int_stack(&[1]);

        test_case(
            "
            ONE
            PUSHINT 1
            PUSHINT 1
            PUSHINT 1
            NEWC
            STU 255
            STU 256
            STU 256
            BCHKBITS 256
        ",
        )
        .expect_int_stack(&[1]);
    }

    #[test]
    fn check_can_bits_stored_in_builder_err_bit_over() {
        test_case(
            "PUSHINT 1
             NEWC
             STU 8
             PUSHINT 1023
             BCHKBITS",
        )
        .expect_failure(ExceptionCode::CellOverflow);
    }

    #[test]
    fn check_can_bits_stored_in_builder_err_no_count() {
        test_case(
            "NEWC
             BCHKBITS",
        )
        .expect_failure(ExceptionCode::StackUnderflow);
    }

    #[test]
    fn check_can_bits_stored_in_builder_err_no_builder() {
        test_case(
            "PUSHINT 8
             BCHKBITS",
        )
        .expect_failure(ExceptionCode::StackUnderflow);
    }
}

mod bchkbitsq {
    use super::*;

    #[test]
    fn check_can_bits_stored_in_builder_quiet() {
        test_case(
            "
            ONE
            NEWC
            PUSHINT 1023
            BCHKBITSQ
        ",
        )
        .expect_int_stack(&[1, -1]);
    }

    #[test]
    fn check_can_bits_stored_in_builder_with_data_quiet() {
        test_case(
            "
            ONE
            PUSHINT 1
            PUSHINT 1
            PUSHINT 1
            NEWC
            STU 255
            STU 256
            STU 256
            BCHKBITSQ 256
        ",
        )
        .expect_int_stack(&[1, -1]);
    }

    #[test]
    fn check_can_bits_stored_in_builder_err_bit_over_quiet() {
        test_case(
            "
            ONE
            PUSHINT 1
            NEWC
            STU 8
            PUSHINT 1023
            BCHKBITSQ
        ",
        )
        .expect_int_stack(&[1, 0]);

        test_case(
            "
            ONE
            PUSHINT 1
            PUSHINT 1
            PUSHINT 1
            NEWC
            STU 256
            STU 256
            STU 256
            BCHKBITSQ 256
        ",
        )
        .expect_int_stack(&[1, 0]);
    }

    #[test]
    fn check_can_bits_stored_in_builder_err_no_arg_quiet() {
        test_case(
            "NEWC
             BCHKBITSQ",
        )
        .expect_failure(ExceptionCode::StackUnderflow);

        test_case(
            "PUSHINT 1
             BCHKBITSQ",
        )
        .expect_failure(ExceptionCode::StackUnderflow);

        test_case("BCHKBITSQ 255").expect_failure(ExceptionCode::StackUnderflow);
    }
}

mod bchkrefs {
    use super::*;

    #[test]
    fn check_can_refs_stored_in_builder() {
        test_case(
            "
            ONE
            NEWC
            PUSHINT 4
            BCHKREFS
        ",
        )
        .expect_int_stack(&[1]);
    }

    #[test]
    fn check_can_refs_stored_in_builder_with_data() {
        test_case(
            "
            ONE
            NEWC
            ENDC
            NEWC
            STREF
            PUSHINT 3
            BCHKREFS
        ",
        )
        .expect_int_stack(&[1]);
    }

    #[test]
    fn check_can_refs_stored_in_builder_err_ref_over() {
        test_case(
            "NEWC
             ENDC
             NEWC
             STREF
             PUSHINT 4
             BCHKREFS",
        )
        .expect_failure(ExceptionCode::CellOverflow);
    }

    #[test]
    fn check_can_refs_stored_in_builder_err_no_count() {
        test_case(
            "NEWC
             BCHKREFS",
        )
        .expect_failure(ExceptionCode::StackUnderflow);
    }

    #[test]
    fn check_can_refs_stored_in_builder_err_no_builder() {
        test_case(
            "PUSHINT 4
             BCHKREFS",
        )
        .expect_failure(ExceptionCode::StackUnderflow);
    }
}

mod bchkrefsq {
    use super::*;

    #[test]
    fn check_can_refs_stored_in_builder_quiet() {
        test_case(
            "
            ONE
            NEWC
            PUSHINT 4
            BCHKREFSQ
        ",
        )
        .expect_int_stack(&[1, -1]);
    }

    #[test]
    fn check_can_refs_stored_in_builder_with_data_quiet() {
        test_case(
            "
            ONE
            NEWC
            ENDC
            NEWC
            STREF
            PUSHINT 3
            BCHKREFSQ
        ",
        )
        .expect_int_stack(&[1, -1]);
    }

    #[test]
    fn check_can_refs_stored_in_builder_err_ref_over_quiet() {
        test_case(
            "
            NEWC
            ENDC
            NEWC
            STREF
            PUSHINT 4
            BCHKREFSQ
        ",
        )
        .expect_int_stack(&[0]);
    }

    #[test]
    fn check_can_refs_stored_in_builder_err_no_count_quiet() {
        test_case(
            "NEWC
             BCHKREFSQ",
        )
        .expect_failure(ExceptionCode::StackUnderflow);
    }

    #[test]
    fn check_can_refs_stored_in_builder_err_no_builder_quiet() {
        test_case(
            "PUSHINT 4
             BCHKREFSQ",
        )
        .expect_failure(ExceptionCode::StackUnderflow);
    }
}

mod bchkbitrefs {
    use super::*;

    #[test]
    fn check_can_refs_and_bits_stored_in_builder() {
        test_case(
            "
            ONE
            NEWC
            PUSHINT 1023
            PUSHINT 4
            BCHKBITREFS
        ",
        )
        .expect_int_stack(&[1]);
    }

    #[test]
    fn check_can_refs_and_bits_stored_in_builder_err_ref_over() {
        test_case(
            "NEWC
             ENDC
             NEWC
             STREF
             PUSHINT 300
             PUSHINT 4
             BCHKBITREFS",
        )
        .expect_failure(ExceptionCode::CellOverflow);
    }

    #[test]
    fn check_can_refs_and_bits_stored_in_builder_err_bit_over() {
        test_case(
            "PUSHINT 1
             NEWC
             STU 8
             PUSHINT 1023
             PUSHINT 1
             BCHKBITREFS",
        )
        .expect_failure(ExceptionCode::CellOverflow);
    }

    #[test]
    fn check_can_refs_and_bits_stored_in_builder_err_ref_and_bit_over() {
        test_case(
            "NEWC
             ENDC
             NEWC
             STREF
             PUSHINT 1
             SWAP
             STU 8
             PUSHINT 1023
             PUSHINT 4
             BCHKBITREFS",
        )
        .expect_failure(ExceptionCode::CellOverflow);
    }

    #[test]
    fn check_can_refs_and_bits_stored_in_builder_err_no_count() {
        test_case(
            "NEWC
             BCHKBITREFS",
        )
        .expect_failure(ExceptionCode::StackUnderflow);
    }

    #[test]
    fn check_can_refs_and_bits_stored_in_builder_err_no_builder() {
        test_case(
            "PUSHINT 4
             BCHKBITREFS",
        )
        .expect_failure(ExceptionCode::StackUnderflow);
    }
}

mod bchkbitrefsq {
    use super::*;

    #[test]
    fn check_can_refs_and_bits_stored_in_builder_quiet() {
        test_case(
            "
            ONE
            NEWC
            PUSHINT 1023
            PUSHINT 4
            BCHKBITREFSQ
        ",
        )
        .expect_int_stack(&[1, -1]);
    }

    #[test]
    fn check_can_refs_and_bits_stored_in_builder_err_ref_over_quiet() {
        test_case(
            "
            ONE
            NEWC
            ENDC
            NEWC
            STREF
            PUSHINT 300
            PUSHINT 4
            BCHKBITREFSQ
        ",
        )
        .expect_int_stack(&[1, 0]);
    }

    #[test]
    fn check_can_refs_and_bits_stored_in_builder_err_bit_over_quiet() {
        test_case(
            "
            ONE
            PUSHINT 1
            NEWC
            STU 8
            PUSHINT 1023
            PUSHINT 1
            BCHKBITREFSQ
        ",
        )
        .expect_int_stack(&[1, 0]);
    }

    #[test]
    fn check_can_refs_and_bits_stored_in_builder_err_ref_and_bit_over_quiet() {
        test_case(
            "
            ONE
            NEWC
            ENDC
            NEWC
            STREF
            PUSHINT 1
            SWAP
            STU 8
            PUSHINT 1023
            PUSHINT 4
            BCHKBITREFSQ
        ",
        )
        .expect_int_stack(&[1, 0]);
    }

    #[test]
    fn check_can_refs_and_bits_stored_in_builder_err_no_count_quiet() {
        test_case(
            "NEWC
             BCHKBITREFSQ",
        )
        .expect_failure(ExceptionCode::StackUnderflow);
    }

    #[test]
    fn check_can_refs_and_bits_stored_in_builder_err_no_builder_quiet() {
        test_case(
            "PUSHINT 4
             BCHKBITREFSQ",
        )
        .expect_failure(ExceptionCode::StackUnderflow);
    }
}

mod stzeroes {
    use super::*;

    #[test]
    fn put_binzeroes_to_builder() {
        test_case(
            "NEWC
             PUSHINT 4
             STZEROES",
        )
        .expect_item(create::builder([0b00001000]));
    }

    #[test]
    fn put_binzeroes_to_builder_err_no_count() {
        test_case(
            "NEWC
             STZEROES",
        )
        .expect_failure(ExceptionCode::StackUnderflow);
    }

    #[test]
    fn put_binzeroes_to_builder_err_no_builder() {
        test_case(
            "PUSHINT 1
             STZEROES",
        )
        .expect_failure(ExceptionCode::StackUnderflow);
    }
}

mod stones {
    use super::*;

    #[test]
    fn put_binones_to_builder() {
        test_case(
            "NEWC
             PUSHINT 4
             STONES",
        )
        .expect_item(create::builder([0b11111000]));
    }

    #[test]
    fn put_binones_to_builder_err_no_count() {
        test_case(
            "NEWC
             STONES",
        )
        .expect_failure(ExceptionCode::StackUnderflow);
    }

    #[test]
    fn put_binones_to_builder_err_no_builder() {
        test_case(
            "PUSHINT 1
             STONES",
        )
        .expect_failure(ExceptionCode::StackUnderflow);
    }
}

mod stsliceconst {
    use super::*;

    #[test]
    fn put_sliceconst_zero() {
        test_case(
            "NEWC
             STSLICECONST 0",
        )
        .expect_item(create::builder([0b01000000]));

        test_case(
            "NEWC
             STZERO",
        )
        .expect_item(create::builder([0b01000000]));
    }

    #[test]
    fn put_sliceconst_one() {
        test_case(
            "NEWC
             STSLICECONST 1",
        )
        .expect_item(create::builder([0b11000000]));

        test_case(
            "NEWC
             STONE",
        )
        .expect_item(create::builder([0b11000000]));
    }

    #[test]
    fn put_sliceconst_data() {
        test_case(
            "
            NEWC
            STSLICECONST x50
        ",
        )
        .expect_item(create::builder([0x50, 0x80]));
    }
}

mod stsame {
    use super::*;

    #[test]
    fn put_same_bin_ones_to_builder() {
        test_case(
            "NEWC
             PUSHINT 4
             PUSHINT 1
             STSAME",
        )
        .expect_item(create::builder([0b11111000]));
    }

    #[test]
    fn put_same_bin_zeroes_to_builder() {
        test_case(
            "NEWC
             PUSHINT 4
             PUSHINT 0
             STSAME",
        )
        .expect_item(create::builder([0b00001000]));
    }

    #[test]
    fn put_same_bin_to_builder_err_no_count_and_same() {
        test_case(
            "NEWC
             STSAME",
        )
        .expect_failure(ExceptionCode::StackUnderflow);
    }

    #[test]
    fn put_same_bin_to_builder_err_no_count_or_same() {
        test_case(
            "NEWC
             PUSHINT 1
             STSAME",
        )
        .expect_failure(ExceptionCode::StackUnderflow);
    }

    #[test]
    fn put_same_bin_to_builder_err_no_builder() {
        test_case(
            "PUSHINT 1
             PUSHINT 1
             STSAME",
        )
        .expect_failure(ExceptionCode::StackUnderflow);
    }
}

mod builder_depth {
    use super::*;

    #[test]
    fn test_builder_depth_normal() {
        test_case(
            "
            NEWC
            BDEPTH
        ",
        )
        .expect_item(int!(0));
        test_case(
            "
            NEWC
            ZERO
            STUR 8
            BDEPTH
        ",
        )
        .expect_item(int!(0));
        test_case(
            "
            NEWC
            NEWC
            STBREFR
            BDEPTH
        ",
        )
        .expect_item(int!(1));
        test_case(
            "
            NEWC
            NEWC
            STBREFR
            NEWC
            STBREFR
            BDEPTH
        ",
        )
        .expect_item(int!(1));
        test_case(
            "
            NEWC
            NEWC
            STBREFR
            NEWC
            NEWC
            STBREFR
            STBREFR
            BDEPTH
        ",
        )
        .expect_item(int!(2));
    }

    #[test]
    fn test_builder_depth_stack_underflow() {
        expect_exception("BDEPTH", ExceptionCode::StackUnderflow);
    }

    #[test]
    fn test_builder_depth_type_check_error() {
        expect_exception("NIL BDEPTH", ExceptionCode::TypeCheckError);
        expect_exception("NULL BDEPTH", ExceptionCode::TypeCheckError);
        expect_exception("ZERO BDEPTH", ExceptionCode::TypeCheckError);
        expect_exception("NEWC ENDC BDEPTH", ExceptionCode::TypeCheckError);
        expect_exception("PUSHCONT {} BDEPTH", ExceptionCode::TypeCheckError);
        expect_exception("PUSHSLICE x0_ BDEPTH", ExceptionCode::TypeCheckError);
    }
}

mod cell_depth {
    use super::*;

    #[test]
    fn test_cell_depth_normal() {
        test_case(
            "
            NULL
            CDEPTH
        ",
        )
        .expect_item(int!(0));
        test_case(
            "
            NEWC
            ENDC
            CDEPTH
        ",
        )
        .expect_item(int!(0));
        test_case(
            "
            NEWC
            NEWC
            STBREFR
            ENDC
            CDEPTH
        ",
        )
        .expect_item(int!(1));
        test_case(
            "
            NEWC
            NEWC
            STBREFR
            NEWC
            STBREFR
            ENDC
            CDEPTH
        ",
        )
        .expect_item(int!(1));
        test_case(
            "
            NEWC
            NEWC
            STBREFR
            NEWC
            NEWC
            STBREFR
            STBREFR
            ENDC
            CDEPTH
        ",
        )
        .expect_item(int!(2));
    }

    #[test]
    fn test_cell_depth_stack_underflow() {
        expect_exception("CDEPTH", ExceptionCode::StackUnderflow);
    }

    #[test]
    fn test_cell_depth_type_check_error() {
        expect_exception("NIL CDEPTH", ExceptionCode::TypeCheckError);
        expect_exception("ZERO CDEPTH", ExceptionCode::TypeCheckError);
        expect_exception("NEWC CDEPTH", ExceptionCode::TypeCheckError);
        expect_exception("PUSHCONT {} CDEPTH", ExceptionCode::TypeCheckError);
        expect_exception("PUSHSLICE x0_ CDEPTH", ExceptionCode::TypeCheckError);
    }
}

mod slice_depth {
    use super::*;

    #[test]
    fn test_slice_depth_normal() {
        test_case(
            "
            PUSHSLICE x0_
            SDEPTH
        ",
        )
        .expect_item(int!(0));
        test_case(
            "
            NEWC
            NEWC
            STBREFR
            ENDC
            CTOS
            SDEPTH
        ",
        )
        .expect_item(int!(1));
        test_case(
            "
            NEWC
            NEWC
            STBREFR
            NEWC
            STBREFR
            ENDC
            CTOS
            SDEPTH
        ",
        )
        .expect_item(int!(1));
        test_case(
            "
            NEWC
            NEWC
            STBREFR
            NEWC
            NEWC
            STBREFR
            STBREFR
            ENDC
            CTOS
            SDEPTH
        ",
        )
        .expect_item(int!(2));
    }

    #[test]
    fn test_slice_depth_stack_underflow() {
        expect_exception("SDEPTH", ExceptionCode::StackUnderflow);
    }

    #[test]
    fn test_slice_depth_type_check_error() {
        expect_exception("NIL SDEPTH", ExceptionCode::TypeCheckError);
        expect_exception("NULL SDEPTH", ExceptionCode::TypeCheckError);
        expect_exception("ZERO SDEPTH", ExceptionCode::TypeCheckError);
        expect_exception("NEWC SDEPTH", ExceptionCode::TypeCheckError);
        expect_exception("NEWC ENDC SDEPTH", ExceptionCode::TypeCheckError);
        expect_exception("PUSHCONT {} SDEPTH", ExceptionCode::TypeCheckError);
    }
}

#[test]
fn test_create_deep_cell() {
    let depth = 1024;
    test_case(format!(
        "
        NEWC
        ENDC
        PUSHINT {}
        PUSHCONT {{
            NEWC
            STREF
            ENDC
        }}
        REPEAT
        CDEPTH
    ",
        depth
    ))
    .expect_int_stack(&[depth]);
}

#[test]
fn test_create_too_deep_cell() {
    test_case(
        "
        NEWC
        ENDC
        PUSHINT 1025
        PUSHCONT {
            NEWC
            STREF
            ENDC
        }
        REPEAT
        CDEPTH
    ",
    )
    .expect_failure(ExceptionCode::CellOverflow);
}

#[test]
fn test_endxc_type_check() {
    for typ in 0..256 {
        match typ {
            1 =>
            // PrunedBranch
            {
                test_case(format!(
                    "
                    NEWC
                    STSLICECONST x{typ:02x}
                    STSLICECONST x01
                    PUSHINT 272 STZEROES
                    TRUE
                    ENDXC
                "
                ))
                .expect_success()
            }
            2 =>
            // LibraryReference
            {
                test_case(format!(
                    "
                    NEWC
                    STSLICECONST x{typ:02x}
                    PUSHINT 256 STZEROES
                    TRUE
                    ENDXC
                "
                ))
                .expect_success()
            }
            3 =>
            // MerkleProof
            {
                test_case(format!(
                    "
                    NEWC
                    STSLICECONST x{typ:02x}
                    PUSHINT 272 STZEROES
                    NEWC STBREFR
                    TRUE
                    ENDXC
                "
                ))
                .expect_failure(ExceptionCode::CellOverflow)
            }
            4 =>
            // MerkleUpdate
            {
                test_case(format!(
                    "
                    NEWC
                    STSLICECONST x{typ:02x}
                    PUSHINT 544 STZEROES
                    NEWC STBREFR
                    NEWC STBREFR
                    TRUE
                    ENDXC
                "
                ))
                .expect_failure(ExceptionCode::CellOverflow)
            }
            _ => test_case(format!(
                "
                    NEWC
                    STSLICECONST x{typ:02x}
                    TRUE
                    ENDXC
                "
            ))
            .expect_failure(ExceptionCode::CellOverflow),
        };
    }
}
