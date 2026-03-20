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
use ton_block::ExceptionCode;
use ton_vm::{
    boolean, int,
    stack::{integer::IntegerData, Stack, StackItem},
};

mod common;
use common::*;

#[test]
fn test_tuple_create_normal() {
    test_case(
        "
        ONE
        TWO
        PUSHSLICE x3_
        PUSHSLICE x4_
        TUPLE 4
        NIL
        ONE
        SINGLE
        ONE
        TWO
        PAIR
        TRUE
        FALSE
        CONS
        ZERO
        ONE
        TWO
        TRIPLE
    ",
    )
    .expect_stack(
        Stack::new()
            .push(create::tuple(&[int!(1), int!(2), create::slice([0x30]), create::slice([0x40])]))
            .push(create::tuple(&[]))
            .push(create::tuple(&[int!(1)]))
            .push(create::tuple(&[int!(1), int!(2)]))
            .push(create::tuple(&[boolean!(true), boolean!(false)]))
            .push(create::tuple(&[int!(0), int!(1), int!(2)])),
    );
}

#[test]
fn test_tuple_create_stack_underflow_error() {
    test_case(
        "
        ONE
        PAIR
    ",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_tuple_createvar_normal() {
    test_case(
        "
        ONE
        TWO
        TWO
        TUPLEVAR
    ",
    )
    .expect_stack(Stack::new().push(create::tuple(&[int!(1), int!(2)])));
}

#[test]
fn test_tuple_createvar_stack_underflow_error() {
    test_case(
        "
        ONE
        TWO
        TUPLEVAR
    ",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_tuple_index_normal() {
    test_case(
        "
        ZERO
        ONE
        TWO
        TRIPLE
        DUP
        INDEX 0
        SWAP
        DUP
        INDEX 1
        SWAP
        INDEX 2
    ",
    )
    .expect_stack(Stack::new().push(int!(0)).push(int!(1)).push(int!(2)));
}

#[test]
fn test_tuple_index_alias() {
    test_case(
        "
        ZERO
        ONE
        TWO
        TRIPLE
        DUP
        FIRST
        SWAP
        DUP
        CAR
        SWAP
        DUP
        SECOND
        SWAP
        DUP
        CDR
        SWAP
        THIRD
    ",
    )
    .expect_stack(
        Stack::new().push(int!(0)).push(int!(0)).push(int!(1)).push(int!(1)).push(int!(2)),
    );
}

#[test]
fn test_tuple_index_range_check_error() {
    test_case(
        "
        ZERO
        ONE
        TWO
        TRIPLE
        INDEX 3
    ",
    )
    .expect_failure(ExceptionCode::RangeCheckError);
}

#[test]
fn test_tuple_index_type_check_error() {
    test_case(
        "
        ZERO
        INDEX 3
    ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);

    test_case(
        "
        NULL
        INDEX 3
    ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

#[test]
fn test_tuple_index_stack_underflow_error() {
    test_case(
        "
        INDEX 3
    ",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_tuple_index_quiet_normal() {
    test_case(
        "
        ZERO
        ONE
        TWO
        TRIPLE
        DUP
        INDEXQ 0
        SWAP
        DUP
        INDEXQ 1
        SWAP
        INDEXQ 2
    ",
    )
    .expect_stack(Stack::new().push(int!(0)).push(int!(1)).push(int!(2)));

    test_case(
        "
        ZERO
        ONE
        TWO
        TRIPLE
        INDEXQ 3
    ",
    )
    .expect_item(StackItem::None);

    test_case(
        "
        NULL
        INDEXQ 3
    ",
    )
    .expect_item(StackItem::None);
}

#[test]
fn test_tuple_index_quiet_type_check_error() {
    test_case(
        "
        ZERO
        INDEXQ 3
    ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

#[test]
fn test_tuple_index_quiet_stack_underflow_error() {
    test_case(
        "
        INDEXQ 3
    ",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_tuple_index2_normal() {
    test_case(
        "
        ZERO
        ONE
        TWO
        TRIPLE
        PUSHINT 3
        PUSHINT 4
        PAIR
        PAIR
        DUP
        INDEX2 0, 0
        SWAP
        DUP
        INDEX2 0, 1
        SWAP
        DUP
        INDEX2 0, 2
        SWAP
        DUP
        INDEX2 1, 0
        SWAP
        INDEX2 1, 1
    ",
    )
    .expect_stack(
        Stack::new().push(int!(0)).push(int!(1)).push(int!(2)).push(int!(3)).push(int!(4)),
    );
}

#[test]
fn test_tuple_index2_alias() {
    test_case(
        "
        ZERO
        ONE
        TWO
        TRIPLE
        PUSHINT 3
        PUSHINT 4
        PAIR
        PAIR
        DUP
        CADR
        SWAP
        CDDR
    ",
    )
    .expect_stack(Stack::new().push(int!(3)).push(int!(4)));
}

#[test]
fn test_tuple_index2_range_check_error() {
    test_case(
        "
        ZERO
        ONE
        TWO
        TRIPLE
        INDEX2 3, 0
    ",
    )
    .expect_failure(ExceptionCode::RangeCheckError);

    test_case(
        "
        ZERO
        ONE
        TWO
        TRIPLE
        PUSHINT 3
        PUSHINT 4
        PAIR
        PAIR
        INDEX2 1, 2
    ",
    )
    .expect_failure(ExceptionCode::RangeCheckError);
}

#[test]
fn test_tuple_index2_type_check_error() {
    test_case(
        "
        NULL
        INDEX2 1, 1
    ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);

    test_case(
        "
        ZERO
        INDEX2 1, 1
    ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);

    test_case(
        "
        ZERO
        ONE
        TWO
        TRIPLE
        INDEX2 0, 0
    ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

#[test]
fn test_tuple_index2_stack_underflow_error() {
    test_case(
        "
        INDEX2 1, 1
    ",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_tuple_index3_normal() {
    test_case(
        "
        PUSHINT -2
        PUSHINT -1
        ZERO
        ONE
        TWO
        TRIPLE
        PUSHINT 3
        PUSHINT 4
        PAIR
        TRIPLE
        PUSHINT 5
        PUSHINT 6
        PAIR
        TRIPLE
        DUP
        INDEX3 1, 1, 0
        SWAP
        DUP
        INDEX3 1, 1, 1
        SWAP
        DUP
        INDEX3 1, 1, 2
        SWAP
        DUP
        INDEX3 1, 2, 0
        SWAP
        INDEX3 1, 2, 1
    ",
    )
    .expect_stack(
        Stack::new().push(int!(0)).push(int!(1)).push(int!(2)).push(int!(3)).push(int!(4)),
    );
}

#[test]
fn test_tuple_index3_alias() {
    test_case(
        "
        PUSHINT -2
        PUSHINT -1
        ZERO
        ONE
        TWO
        TRIPLE
        PUSHINT 3
        PUSHINT 4
        PAIR
        TRIPLE
        PUSHINT 5
        PUSHINT 6
        PAIR
        TRIPLE
        DUP
        CADDR
        SWAP
        CDDDR
    ",
    )
    .expect_stack(Stack::new().push(int!(0)).push(int!(1)));
}

#[test]
fn test_tuple_index3_range_check_error() {
    test_case(
        "
        ZERO
        ONE
        TWO
        TRIPLE
        INDEX3 3, 0, 0
    ",
    )
    .expect_failure(ExceptionCode::RangeCheckError);

    test_case(
        "
        ZERO
        ONE
        TWO
        TRIPLE
        PUSHINT 3
        PUSHINT 4
        PAIR
        PAIR
        INDEX3 1, 2, 0
    ",
    )
    .expect_failure(ExceptionCode::RangeCheckError);

    test_case(
        "
        PUSHINT -2
        PUSHINT -1
        ZERO
        ONE
        TWO
        TRIPLE
        PUSHINT 3
        PUSHINT 4
        PAIR
        TRIPLE
        PUSHINT 5
        PUSHINT 6
        PAIR
        TRIPLE
        INDEX3 1, 1, 3
    ",
    )
    .expect_failure(ExceptionCode::RangeCheckError);
}

#[test]
fn test_tuple_index3_type_check_error() {
    test_case(
        "
        NULL
        INDEX3 1, 1, 1
    ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);

    test_case(
        "
        ZERO
        INDEX3 1, 1, 1
    ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);

    test_case(
        "
        ZERO
        ONE
        TWO
        TRIPLE
        INDEX3 0, 0, 0
    ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

#[test]
fn test_tuple_index3_stack_underflow_error() {
    test_case(
        "
        INDEX3 1, 1, 1
    ",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_tuple_indexvar_normal() {
    test_case(
        "
        ZERO
        ONE
        TWO
        TRIPLE
        DUP
        ZERO
        INDEXVAR
        SWAP
        DUP
        ONE
        INDEXVAR
        SWAP
        TWO
        INDEXVAR
    ",
    )
    .expect_stack(Stack::new().push(int!(0)).push(int!(1)).push(int!(2)));
}

#[test]
fn test_tuple_indexvar_range_check_error() {
    test_case(
        "
        NIL
        PUSHINT 255
        INDEXVAR
    ",
    )
    .expect_failure(ExceptionCode::RangeCheckError);

    test_case(
        "
        ZERO
        ONE
        TWO
        TRIPLE
        PUSHINT 3
        INDEXVAR
    ",
    )
    .expect_failure(ExceptionCode::RangeCheckError);
}

#[test]
fn test_tuple_indexvar_type_check_error() {
    test_case(
        "
        ZERO
        TWO
        INDEXVAR
    ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);

    test_case(
        "
        NULL
        TWO
        INDEXVAR
    ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

#[test]
fn test_tuple_indexvar_stack_underflow_error() {
    test_case(
        "
        INDEXVAR
    ",
    )
    .expect_failure(ExceptionCode::StackUnderflow);

    test_case(
        "
        TWO
        INDEXVAR
    ",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_tuple_indexvar_quiet_normal() {
    test_case(
        "
        ZERO
        ONE
        TWO
        TRIPLE
        DUP
        ZERO
        INDEXVARQ
        SWAP
        DUP
        ONE
        INDEXVARQ
        SWAP
        TWO
        INDEXVARQ
    ",
    )
    .expect_stack(Stack::new().push(int!(0)).push(int!(1)).push(int!(2)));

    test_case(
        "
        ZERO
        ONE
        TWO
        TRIPLE
        PUSHINT 3
        INDEXVARQ
    ",
    )
    .expect_item(StackItem::None);

    test_case(
        "
        NULL
        TWO
        INDEXVARQ
    ",
    )
    .expect_item(StackItem::None);
}

#[test]
fn test_tuple_indexvar_quiet_range_check_error() {
    test_case(
        "
        NIL
        PUSHINT 255
        INDEXVARQ
    ",
    )
    .expect_failure(ExceptionCode::RangeCheckError);
}

#[test]
fn test_tuple_indexvar_quiet_type_check_error() {
    test_case(
        "
        ZERO
        TWO
        INDEXVARQ
    ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

#[test]
fn test_tuple_indexvar_quiet_stack_underflow_error() {
    test_case(
        "
        INDEXVARQ
    ",
    )
    .expect_failure(ExceptionCode::StackUnderflow);

    test_case(
        "
        TWO
        INDEXVARQ
    ",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_untuple_normal() {
    test_case(
        "
        ZERO
        ONE
        TWO
        TRIPLE
        UNTUPLE 3
    ",
    )
    .expect_stack(Stack::new().push(int!(0)).push(int!(1)).push(int!(2)));
}

#[test]
fn test_untuple_stack_underflow_error() {
    test_case(
        "
        UNTUPLE 2
    ",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_untuple_type_check_error() {
    test_case(
        "
        ZERO
        ONE
        TWO
        TRIPLE
        UNTUPLE 2
    ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);

    test_case(
        "
        ZERO
        UNTUPLE 2
    ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);

    test_case(
        "
        NULL
        UNTUPLE 2
    ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

#[test]
fn test_untuplevar_normal() {
    test_case(
        "
        ZERO
        ONE
        TWO
        TRIPLE
        PUSHINT 3
        UNTUPLEVAR
    ",
    )
    .expect_stack(Stack::new().push(int!(0)).push(int!(1)).push(int!(2)));
}

#[test]
fn test_untuplevar_stack_underflow_error() {
    test_case(
        "
        UNTUPLEVAR
    ",
    )
    .expect_failure(ExceptionCode::StackUnderflow);

    test_case(
        "
        TWO
        UNTUPLEVAR
    ",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_untuplevar_type_check_error() {
    test_case(
        "
        ZERO
        ONE
        TWO
        TRIPLE
        TWO
        UNTUPLEVAR
    ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);

    test_case(
        "
        ZERO
        TWO
        UNTUPLEVAR
    ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);

    test_case(
        "
        NULL
        TWO
        UNTUPLEVAR
    ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

#[test]
fn test_unpackfirst_normal() {
    test_case(
        "
        ZERO
        ONE
        TWO
        TRIPLE
        UNPACKFIRST 2
    ",
    )
    .expect_stack(Stack::new().push(int!(0)).push(int!(1)));
}

#[test]
fn test_unpackfirst_alias() {
    test_case(
        "
        ZERO
        ONE
        TWO
        PAIR
        CHKTUPLE
    ",
    )
    .expect_stack(Stack::new().push(int!(0)));
}

#[test]
fn test_unpackfirst_stack_underflow_error() {
    test_case(
        "
        UNPACKFIRST 2
    ",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_unpackfirst_type_check_error() {
    test_case(
        "
        ZERO
        ONE
        TWO
        TRIPLE
        UNPACKFIRST 4
    ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);

    test_case(
        "
        ZERO
        UNPACKFIRST 2
    ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);

    test_case(
        "
        NULL
        UNPACKFIRST 2
    ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

#[test]
fn test_unpackfirstvar_normal() {
    test_case(
        "
        ZERO
        ONE
        TWO
        TRIPLE
        TWO
        UNPACKFIRSTVAR
    ",
    )
    .expect_stack(Stack::new().push(int!(0)).push(int!(1)));
}

#[test]
fn test_unpackfirstvar_stack_underflow_error() {
    test_case(
        "
        UNPACKFIRSTVAR
    ",
    )
    .expect_failure(ExceptionCode::StackUnderflow);

    test_case(
        "
        TWO
        UNPACKFIRSTVAR
    ",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_unpackfirstvar_type_check_error() {
    test_case(
        "
        ZERO
        ONE
        TWO
        TRIPLE
        PUSHINT 4
        UNPACKFIRSTVAR
    ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);

    test_case(
        "
        ZERO
        TWO
        UNPACKFIRSTVAR
    ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);

    test_case(
        "
        NULL
        TWO
        UNPACKFIRSTVAR
    ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

#[test]
fn test_explode_normal() {
    test_case(
        "
        ZERO
        ONE
        TWO
        TRIPLE
        EXPLODE 4
    ",
    )
    .expect_stack(Stack::new().push(int!(0)).push(int!(1)).push(int!(2)).push(int!(3)));
}

#[test]
fn test_explode_stack_underflow_error() {
    test_case(
        "
        EXPLODE 2
    ",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_explode_type_check_error() {
    test_case(
        "
        ZERO
        ONE
        TWO
        TRIPLE
        EXPLODE 2
    ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);

    test_case(
        "
        ZERO
        EXPLODE 2
    ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);

    test_case(
        "
        NULL
        EXPLODE 2
    ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

#[test]
fn test_explodevar_normal() {
    test_case(
        "
        ZERO
        ONE
        TWO
        TRIPLE
        PUSHINT 4
        EXPLODEVAR
    ",
    )
    .expect_stack(Stack::new().push(int!(0)).push(int!(1)).push(int!(2)).push(int!(3)));
}

#[test]
fn test_explodevar_stack_underflow_error() {
    test_case(
        "
        EXPLODEVAR
    ",
    )
    .expect_failure(ExceptionCode::StackUnderflow);

    test_case(
        "
        PUSHINT 2
        EXPLODEVAR
    ",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_explodevar_type_check_error() {
    test_case(
        "
        ZERO
        ONE
        TWO
        TRIPLE
        TWO
        EXPLODEVAR
    ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);

    test_case(
        "
        ZERO
        TWO
        EXPLODEVAR
    ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);

    test_case(
        "
        NULL
        TWO
        EXPLODEVAR
    ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

#[test]
fn test_setindex_normal() {
    test_case(
        "
        ZERO
        ONE
        TWO
        TRIPLE
        TEN
        SETINDEX 0
    ",
    )
    .expect_stack(Stack::new().push(create::tuple(&[int!(10), int!(1), int!(2)])));
}

#[test]
fn test_setindex_range_check_error() {
    test_case(
        "
        ZERO
        ONE
        TWO
        TRIPLE
        TEN
        SETINDEX 3
    ",
    )
    .expect_failure(ExceptionCode::RangeCheckError);
}

#[test]
fn test_setindex_type_check_error() {
    test_case(
        "
        ONE
        TWO
        SETINDEX 3
    ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);

    test_case(
        "
        NULL
        TWO
        SETINDEX 3
    ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

#[test]
fn test_setindex_stack_underflow_error() {
    test_case(
        "
        SETINDEX 3
    ",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_setindex_quiet_normal() {
    test_case(
        "
        ZERO
        ONE
        TWO
        TRIPLE
        TEN
        SETINDEXQ 0
    ",
    )
    .expect_stack(Stack::new().push(create::tuple(&[int!(10), int!(1), int!(2)])));

    test_case(
        "
        ZERO
        ONE
        TWO
        TRIPLE
        TEN
        SETINDEXQ 4
    ",
    )
    .expect_stack(Stack::new().push(create::tuple(&[
        int!(0),
        int!(1),
        int!(2),
        StackItem::None,
        int!(10),
    ])));

    test_case(
        "
        NIL
        TWO
        SETINDEXQ 3
    ",
    )
    .expect_stack(Stack::new().push(create::tuple(&[
        StackItem::None,
        StackItem::None,
        StackItem::None,
        int!(2),
    ])));

    test_case(
        "
        NULL
        TWO
        SETINDEXQ 3
    ",
    )
    .expect_stack(Stack::new().push(create::tuple(&[
        StackItem::None,
        StackItem::None,
        StackItem::None,
        int!(2),
    ])));
}

#[test]
fn test_setindex_quiet_type_check_error() {
    test_case(
        "
        ONE
        TWO
        SETINDEXQ 3
    ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

#[test]
fn test_setindex_quiet_stack_underflow_error() {
    test_case(
        "
        SETINDEXQ 3
    ",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_setindexvar_normal() {
    test_case(
        "
        NIL
        ZERO
        PUSHINT 255
        PUSHCONT {
            DUP
            INC
            ROTREV
            TPUSH
            SWAP
        }
        REPEAT
        DROP
        PUSHINT 77
        PUSHINT 254
        SETINDEXVAR
        LAST
    ",
    )
    .expect_int_stack(&[77]);
}

#[test]
fn test_setindexvar_range_check_error() {
    test_case(
        "
        NIL
        PUSHINT 77
        PUSHINT 255
        SETINDEXVAR
    ",
    )
    .expect_failure(ExceptionCode::RangeCheckError);

    test_case(
        "
        ZERO
        ONE
        TWO
        TRIPLE
        TEN
        PUSHINT 3
        SETINDEXVAR
    ",
    )
    .expect_failure(ExceptionCode::RangeCheckError);
}

#[test]
fn test_setindexvar_type_check_error() {
    test_case(
        "
        ONE
        TWO
        PUSHINT 3
        SETINDEXVAR
    ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);

    test_case(
        "
        NULL
        TWO
        PUSHINT 3
        SETINDEXVAR
    ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

#[test]
fn test_setindexvar_stack_underflow_error() {
    test_case(
        "
        SETINDEXVAR
    ",
    )
    .expect_failure(ExceptionCode::StackUnderflow);

    test_case(
        "
        NULL
        SETINDEXVAR
    ",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_setindexvar_quiet_normal() {
    test_case(
        "
        NIL
        PUSHINT 77
        PUSHINT 254
        SETINDEXVARQ
        LAST
    ",
    )
    .expect_int_stack(&[77]);

    test_case(
        "
        NULL
        PUSHINT 77
        PUSHINT 254
        SETINDEXVARQ
        LAST
    ",
    )
    .expect_int_stack(&[77]);
}

#[test]
fn test_setindexvar_quiet_range_check_error() {
    test_case(
        "
        NIL
        PUSHINT 77
        PUSHINT 255
        SETINDEXVARQ
    ",
    )
    .expect_failure(ExceptionCode::RangeCheckError);
}

#[test]
fn test_setindexvar_quiet_type_check_error() {
    test_case(
        "
        ONE
        TWO
        PUSHINT 3
        SETINDEXVARQ
    ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

#[test]
fn test_setindexvar_quiet_stack_underflow_error() {
    test_case(
        "
        SETINDEXVARQ
    ",
    )
    .expect_failure(ExceptionCode::StackUnderflow);

    test_case(
        "
        NULL
        SETINDEXVARQ
    ",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_tuple_len_normal() {
    test_case(
        "
        ZERO
        ONE
        TWO
        TRIPLE
        TLEN
        NIL
        TLEN
    ",
    )
    .expect_int_stack(&[3, 0]);
}

#[test]
fn test_tuple_len_type_check_error() {
    test_case(
        "
        ZERO
        TLEN
    ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);

    test_case(
        "
        NULL
        TLEN
    ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

#[test]
fn test_tuple_len_stack_underflow_error() {
    test_case(
        "
        TLEN
    ",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_tuple_len_quiet_normal() {
    test_case(
        "
        ZERO
        ONE
        TWO
        TRIPLE
        QTLEN
        NIL
        QTLEN
        ONE
        QTLEN
        NULL
        QTLEN
    ",
    )
    .expect_int_stack(&[3, 0, -1, -1]);
}

#[test]
fn test_tuple_len_quiet_stack_underflow_error() {
    test_case(
        "
        QTLEN
    ",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_tuple_last_type_check_error() {
    test_case(
        "
        NIL
        LAST
    ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);

    test_case(
        "
        NULL
        LAST
    ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

#[test]
fn test_tuple_last_stack_underflow_error() {
    test_case(
        "
        LAST
    ",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_tuple_pop_type_check_error() {
    test_case(
        "
        NIL
        TPOP
    ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);

    test_case(
        "
        NULL
        TPOP
    ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

#[test]
fn test_tuple_pop_stack_underflow_error() {
    test_case(
        "
        TPOP
    ",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_tuple_push_type_check_error() {
    test_case(
        "
        NULL
        ONE
        TPUSH
    ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);

    test_case(
        "
        TWO
        ONE
        TPUSH
    ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);

    test_case(
        "
        NIL
        ZERO
        PUSHINT 256
        PUSHCONT {
            DUP
            INC
            ROTREV
            TPUSH
            SWAP
        }
        REPEAT
        FALSE
    ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);
}

#[test]
fn test_tuple_push_stack_underflow_error() {
    test_case(
        "
        TPUSH
    ",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
}

#[test]
fn test_istulpe_normal() {
    test_case(
        "
        ZERO
        ONE
        TWO
        TRIPLE
        ISTUPLE
        ZERO
        ISTUPLE
    ",
    )
    .expect_int_stack(&[-1, 0]);
}

#[test]
fn test_istuple_stack_underflow_error() {
    test_case(
        "
        ISTUPLE
    ",
    )
    .expect_failure(ExceptionCode::StackUnderflow);
}
