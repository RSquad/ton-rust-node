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
use ton_block::{ExceptionCode, SliceData};
use ton_vm::stack::{Stack, StackItem};

mod common;
use common::*;

fn to_cell<T>(data: T) -> StackItem
where
    T: AsRef<[u8]>,
{
    create::cell(data)
}

fn dict_remainder(data: u8) -> StackItem {
    let mut slice = SliceData::new(vec![data]);
    slice.get_next_bit().unwrap();
    StackItem::Slice(slice)
}

#[test]
fn test_dict_is_empty() {
    test_case(
        "
        NEWDICT
        DICTEMPTY
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(true)));
    test_case(
        "
        PUSHSLICE x5_
        PUSHSLICE x7_
        NEWDICT
        PUSHINT 3
        DICTSET
        DICTEMPTY
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)));
}

#[test]
fn test_dict_get_on_empty_dict() {
    test_case(
        "
        PUSHSLICE x5_
        NEWDICT
        PUSHINT 3
        DICTGET
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)));
}

#[test]
fn test_dict_fill_and_get_once() {
    test_case(
        "
        PUSHSLICE x5_
        PUSHSLICE x7_
        NEWDICT
        PUSHINT 3
        DICTSET

        PUSHSLICE x7_
        SWAP
        PUSHINT 3
        DICTGET
        ",
    )
    .expect_stack(Stack::new().push(create::slice([0x50])).push(StackItem::boolean(true)));
}

#[test]
fn test_dict_fill_and_getref_once() {
    test_case(
        "
        PUSHSLICE x5_
        NEWC
        STSLICE
        ENDC
        PUSHSLICE x7_
        NEWDICT
        PUSHINT 3
        DICTSETREF

        PUSHSLICE x7_
        SWAP
        PUSHINT 3
        DICTGETREF
        ",
    )
    .expect_stack(Stack::new().push(to_cell([0x50])).push(StackItem::boolean(true)));
}

#[test]
fn test_dict_fill_keywriter_to_int_and_uget_once() {
    test_case(
        "
        PUSHSLICE x5_
        PUSHINT 120
        NEWDICT
        PUSHINT 8
        DICTISET

        PUSHINT 120
        SWAP
        PUSHINT 8
        DICTIGET
        ",
    )
    .expect_stack(Stack::new().push(create::slice([0x50])).push(StackItem::boolean(true)));

    test_case(
        "
        PUSHSLICE x5_
        PUSHINT -120
        NEWDICT
        PUSHINT 8
        DICTISET

        PUSHINT -120
        SWAP
        PUSHINT 8
        DICTIGET
        ",
    )
    .expect_stack(Stack::new().push(create::slice([0x50])).push(StackItem::boolean(true)));
}

#[test]
fn test_dict_fill_keywriter_to_int_and_ugetref_once() {
    test_case(
        "
        PUSHSLICE x5_
        NEWC
        STSLICE
        ENDC
        PUSHINT 125
        NEWDICT
        PUSHINT 8
        DICTISETREF

        PUSHINT 125
        SWAP
        PUSHINT 8
        DICTIGETREF
        ",
    )
    .expect_stack(Stack::new().push(create::cell([0x50])).push(StackItem::boolean(true)));

    test_case(
        "
        PUSHSLICE x5_
        NEWC
        STSLICE
        ENDC
        PUSHINT -125
        NEWDICT
        PUSHINT 8
        DICTISETREF

        PUSHINT -125
        SWAP
        PUSHINT 8
        DICTIGETREF
        ",
    )
    .expect_stack(Stack::new().push(create::cell([0x50])).push(StackItem::boolean(true)));
}

#[test]
fn test_dict_fill_keywriter_to_uint_and_uget_once() {
    test_case(
        "
        PUSHSLICE x5_
        PUSHINT 120
        NEWDICT
        PUSHINT 8
        DICTUSET

        PUSHINT 120
        SWAP
        PUSHINT 8
        DICTUGET
        ",
    )
    .expect_stack(Stack::new().push(create::slice([0x50])).push(StackItem::boolean(true)));
}

#[test]
fn test_dict_fill_keywriter_to_uint_and_ugetref_once() {
    test_case(
        "
        PUSHSLICE x5_
        NEWC
        STSLICE
        ENDC
        PUSHINT 125
        NEWDICT
        PUSHINT 8
        DICTUSETREF

        PUSHINT 125
        SWAP
        PUSHINT 8
        DICTUGETREF
        ",
    )
    .expect_stack(Stack::new().push(to_cell([0x50])).push(StackItem::boolean(true)));
}

#[test]
fn test_dict_fill_and_get_once_with_wrong_key() {
    test_case(
        "
        PUSHSLICE x5
        PUSHSLICE x7_
        NEWDICT
        PUSHINT 3
        DICTSET

        PUSHSLICE x4
        SWAP
        PUSHINT 3
        DICTGET
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)));
}

#[test]
fn test_dict_fill_and_getref_once_with_wrong_key() {
    test_case(
        "
        PUSHSLICE x5
        NEWC
        STSLICE
        ENDC
        PUSHSLICE x7
        NEWDICT
        PUSHINT 3
        DICTSETREF

        PUSHSLICE x4
        SWAP
        PUSHINT 3
        DICTGETREF
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)));
}

#[test]
fn test_dict_fill_keywriter_to_int_and_iget_once_with_wrong_key() {
    test_case(
        "
        PUSHSLICE x5_
        PUSHINT 120
        NEWDICT
        PUSHINT 8
        DICTISET

        PUSHINT 121
        SWAP
        PUSHINT 8
        DICTIGET
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)));
}

#[test]
fn test_dict_fill_keywriter_to_int_and_igetref_once_with_wrong_key() {
    test_case(
        "
        PUSHSLICE x5_
        NEWC
        STSLICE
        ENDC
        PUSHINT 125
        NEWDICT
        PUSHINT 8
        DICTISETREF

        PUSHINT 126
        SWAP
        PUSHINT 8
        DICTIGETREF
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)));
}

#[test]
fn test_dict_fill_keywriter_to_uint_and_uget_once_with_wrong_key() {
    test_case(
        "
        PUSHSLICE x5_
        PUSHINT 120
        NEWDICT
        PUSHINT 8
        DICTUSET

        PUSHINT 121
        SWAP
        PUSHINT 8
        DICTUGET
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)));
}

#[test]
fn test_dict_fill_keywriter_to_uint_and_ugetref_once_with_wrong_key() {
    test_case(
        "
        PUSHSLICE x5_
        NEWC
        STSLICE
        ENDC
        PUSHINT 125
        NEWDICT
        PUSHINT 8
        DICTUSETREF

        PUSHINT 126
        SWAP
        PUSHINT 8
        DICTUGETREF
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)));
}

#[test]
fn test_dict_setget_on_empty_dict() {
    test_case(
        "
        PUSHSLICE x5_
        PUSHSLICE x7_
        NEWDICT
        PUSHINT 3
        DICTSETGET
        SWAP
        PUSHSLICE x7_
        SWAP
        PUSHINT 3
        DICTGET
        ",
    )
    .expect_stack(
        Stack::new()
            .push(StackItem::boolean(false))
            .push(create::slice([0x50]))
            .push(StackItem::boolean(true)),
    );
}

#[test]
fn test_dict_fill_and_setget_once() {
    test_case(
        "
        PUSHSLICE x5_
        PUSHSLICE x7_
        NEWDICT
        PUSHINT 3
        DICTSET

        PUSHSLICE x7_
        SWAP
        PUSHSLICE x7_
        SWAP
        PUSHINT 3
        DICTSETGET

        XCHG s2
        PUSHSLICE x7_
        SWAP
        PUSHINT 3
        DICTGET
        ",
    )
    .expect_stack(
        Stack::new()
            .push(StackItem::boolean(true))
            .push(create::slice([0x50]))
            .push(create::slice([0x70]))
            .push(StackItem::boolean(true)),
    );
}

#[test]
fn test_dict_fill_and_setgetref_once() {
    test_case(
        "
        PUSHSLICE x5_
        NEWC
        STSLICE
        ENDC
        PUSHSLICE x7_
        NEWDICT
        PUSHINT 3
        DICTSETREF

        PUSHSLICE x7_
        NEWC
        STSLICE
        ENDC
        SWAP
        PUSHSLICE x7_
        SWAP
        PUSHINT 3
        DICTSETGETREF

        XCHG s2
        PUSHSLICE x7_
        SWAP
        PUSHINT 3
        DICTGETREF
        ",
    )
    .expect_stack(
        Stack::new()
            .push(StackItem::boolean(true))
            .push(to_cell([0x50]))
            .push(to_cell([0x70]))
            .push(StackItem::boolean(true)),
    );
}

#[test]
fn test_dict_fill_keywriter_to_int_and_isetget_once() {
    test_case(
        "
        PUSHSLICE x5_
        PUSHINT 125
        NEWDICT
        PUSHINT 8
        DICTISET

        PUSHSLICE x7_
        SWAP
        PUSHINT 125
        SWAP
        PUSHINT 8
        DICTISETGET

        XCHG s2
        PUSHINT 125
        SWAP
        PUSHINT 8
        DICTIGET
        ",
    )
    .expect_stack(
        Stack::new()
            .push(StackItem::boolean(true))
            .push(create::slice([0x50]))
            .push(create::slice([0x70]))
            .push(StackItem::boolean(true)),
    );

    test_case(
        "
        PUSHSLICE x5_
        PUSHINT -125
        NEWDICT
        PUSHINT 8
        DICTISET

        PUSHSLICE x7_
        SWAP
        PUSHINT -125
        SWAP
        PUSHINT 8
        DICTISETGET

        XCHG s2
        PUSHINT -125
        SWAP
        PUSHINT 8
        DICTIGET
        ",
    )
    .expect_stack(
        Stack::new()
            .push(StackItem::boolean(true))
            .push(create::slice([0x50]))
            .push(create::slice([0x70]))
            .push(StackItem::boolean(true)),
    );
}

#[test]
fn test_dict_fill_keywriter_to_int_and_isetgetref_once() {
    test_case(
        "
        PUSHSLICE x5_
        NEWC
        STSLICE
        ENDC
        PUSHINT -93
        NEWDICT
        PUSHINT 8
        DICTISETREF

        PUSHSLICE x7_
        NEWC
        STSLICE
        ENDC
        SWAP
        PUSHINT -93
        SWAP
        PUSHINT 8
        DICTISETGETREF

        XCHG s2
        PUSHINT -93
        SWAP
        PUSHINT 8
        DICTIGETREF
        ",
    )
    .expect_stack(
        Stack::new()
            .push(StackItem::boolean(true))
            .push(create::cell([0x50]))
            .push(create::cell([0x70]))
            .push(StackItem::boolean(true)),
    );
}

#[test]
fn test_dict_fill_keywriter_to_uint_and_usetget_once() {
    test_case(
        "
        PUSHSLICE x5_
        PUSHINT 125
        NEWDICT
        PUSHINT 8
        DICTUSET

        PUSHSLICE x7_
        SWAP
        PUSHINT 125
        SWAP
        PUSHINT 8
        DICTUSETGET

        XCHG s2
        PUSHINT 125
        SWAP
        PUSHINT 8
        DICTUGET
        ",
    )
    .expect_stack(
        Stack::new()
            .push(StackItem::boolean(true))
            .push(create::slice([0x50]))
            .push(create::slice([0x70]))
            .push(StackItem::boolean(true)),
    );
}

#[test]
fn test_dict_fill_keywriter_to_uint_and_usetgetref_once() {
    test_case(
        "
        PUSHSLICE x5_
        NEWC
        STSLICE
        ENDC
        PUSHINT 93
        NEWDICT
        PUSHINT 8
        DICTUSETREF

        PUSHSLICE x7_
        NEWC
        STSLICE
        ENDC
        SWAP
        PUSHINT 93
        SWAP
        PUSHINT 8
        DICTUSETGETREF

        XCHG s2
        PUSHINT 93
        SWAP
        PUSHINT 8
        DICTUGETREF
        ",
    )
    .expect_stack(
        Stack::new()
            .push(StackItem::boolean(true))
            .push(to_cell([0x50]))
            .push(to_cell([0x70]))
            .push(StackItem::boolean(true)),
    );
}

#[test]
fn test_dict_replace_once() {
    test_case(
        "
        PUSHSLICE x5_
        PUSHSLICE x7_
        NEWDICT
        PUSHINT 3
        DICTREPLACE

        SWAP
        DROP
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)));

    test_case(
        "
        PUSHSLICE x4_

        PUSHSLICE x5_
        PUSHSLICE x7_
        NEWDICT
        PUSHINT 3
        DICTSET

        PUSHSLICE x7_
        SWAP
        PUSHINT 3
        DICTREPLACE

        SWAP
        DROP
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(true)));
}

#[test]
fn test_dict_replaceref_once() {
    test_case(
        "
        PUSHSLICE x5_
        NEWC
        STSLICE
        ENDC
        PUSHSLICE x7_
        NEWDICT
        PUSHINT 3
        DICTREPLACEREF

        SWAP
        DROP
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)));

    test_case(
        "
        PUSHSLICE x4_
        NEWC
        STSLICE
        ENDC

        PUSHSLICE x5_
        NEWC
        STSLICE
        ENDC
        PUSHSLICE x7_
        NEWDICT
        PUSHINT 3
        DICTSETREF

        PUSHSLICE x7_
        SWAP
        PUSHINT 3
        DICTREPLACEREF

        SWAP
        DROP
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(true)));
}

#[test]
fn test_dict_replace_once_keywriter_to_int() {
    test_case(
        "
        PUSHSLICE x5_
        PUSHINT 99
        NEWDICT
        PUSHINT 8
        DICTIREPLACE

        SWAP
        DROP
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)));

    test_case(
        "
        PUSHSLICE x4_

        PUSHSLICE x5_
        PUSHINT 99
        NEWDICT
        PUSHINT 8
        DICTISET

        PUSHINT 99
        SWAP
        PUSHINT 8
        DICTIREPLACE

        SWAP
        DROP
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(true)));
}

#[test]
fn test_dict_replaceref_once_keywriter_to_int() {
    test_case(
        "
        PUSHSLICE x5_
        NEWC
        STSLICE
        ENDC
        PUSHINT 99
        NEWDICT
        PUSHINT 8
        DICTIREPLACEREF

        SWAP
        DROP
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)));

    test_case(
        "
        PUSHSLICE x4_
        NEWC
        STSLICE
        ENDC

        PUSHSLICE x5_
        NEWC
        STSLICE
        ENDC
        PUSHINT 99
        NEWDICT
        PUSHINT 8
        DICTISETREF

        PUSHINT 99
        SWAP
        PUSHINT 8
        DICTIREPLACEREF

        SWAP
        DROP
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(true)));
}

#[test]
fn test_dict_replace_once_keywriter_to_uint() {
    test_case(
        "
        PUSHSLICE x5_
        PUSHINT 99
        NEWDICT
        PUSHINT 8
        DICTUREPLACE

        SWAP
        DROP
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)));

    test_case(
        "
        PUSHSLICE x4_

        PUSHSLICE x5_
        PUSHINT 99
        NEWDICT
        PUSHINT 8
        DICTUSET

        PUSHINT 99
        SWAP
        PUSHINT 8
        DICTUREPLACE

        SWAP
        DROP
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(true)));
}

#[test]
fn test_dict_replaceref_once_keywriter_to_uint() {
    test_case(
        "
        PUSHSLICE x5_
        NEWC
        STSLICE
        ENDC
        PUSHINT 99
        NEWDICT
        PUSHINT 8
        DICTUREPLACEREF

        SWAP
        DROP
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)));

    test_case(
        "
        PUSHSLICE x4_
        NEWC
        STSLICE
        ENDC

        PUSHSLICE x5_
        NEWC
        STSLICE
        ENDC
        PUSHINT 99
        NEWDICT
        PUSHINT 8
        DICTUSETREF

        PUSHINT 99
        SWAP
        PUSHINT 8
        DICTUREPLACEREF

        SWAP
        DROP
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(true)));
}

#[test]
fn test_dict_replaceget_once() {
    test_case(
        "
        PUSHSLICE x5_
        PUSHSLICE x7_
        NEWDICT
        PUSHINT 3
        DICTREPLACEGET

        SWAP
        DROP
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)));

    test_case(
        "
        PUSHSLICE x4_

        PUSHSLICE x5_
        PUSHSLICE x7_
        NEWDICT
        PUSHINT 3
        DICTSET

        PUSHSLICE x7_
        SWAP
        PUSHINT 3
        DICTREPLACEGET

        XCHG s2
        DROP
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(true)).push(create::slice([0x50])));
}

#[test]
fn test_dict_replacegetref_once() {
    test_case(
        "
        PUSHSLICE x5_
        NEWC
        STSLICE
        ENDC
        PUSHSLICE x7_
        NEWDICT
        PUSHINT 3
        DICTREPLACEGETREF

        SWAP
        DROP
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)));

    test_case(
        "
        PUSHSLICE x4_
        NEWC
        STSLICE
        ENDC

        PUSHSLICE x5_
        NEWC
        STSLICE
        ENDC
        PUSHSLICE x7_
        NEWDICT
        PUSHINT 3
        DICTSETREF

        PUSHSLICE x7_
        SWAP
        PUSHINT 3
        DICTREPLACEGETREF

        XCHG s2
        DROP
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(true)).push(to_cell([0x50])));
}

#[test]
fn test_dict_replaceget_once_keywriter_to_int() {
    test_case(
        "
        PUSHSLICE x5_
        PUSHINT 99
        NEWDICT
        PUSHINT 8
        DICTIREPLACEGET

        SWAP
        DROP
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)));

    test_case(
        "
        PUSHSLICE x4_

        PUSHSLICE x5_
        PUSHINT 99
        NEWDICT
        PUSHINT 8
        DICTISET

        PUSHINT 99
        SWAP
        PUSHINT 8
        DICTIREPLACEGET

        XCHG s2
        DROP
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(true)).push(create::slice([0x50])));
}

#[test]
fn test_dict_replacegetref_once_keywriter_to_int() {
    test_case(
        "
        PUSHSLICE x5_
        NEWC
        STSLICE
        ENDC
        PUSHINT 99
        NEWDICT
        PUSHINT 8
        DICTIREPLACEGETREF

        SWAP
        DROP
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)));

    test_case(
        "
        PUSHSLICE x4_
        NEWC
        STSLICE
        ENDC

        PUSHSLICE x5_
        NEWC
        STSLICE
        ENDC
        PUSHINT 99
        NEWDICT
        PUSHINT 8
        DICTISETREF

        PUSHINT 99
        SWAP
        PUSHINT 8
        DICTIREPLACEGETREF

        XCHG s2
        DROP
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(true)).push(to_cell([0x50])));
}

#[test]
fn test_dict_replaceget_once_keywriter_to_uint() {
    test_case(
        "
        PUSHSLICE x5_
        PUSHINT 99
        NEWDICT
        PUSHINT 8
        DICTUREPLACEGET

        SWAP
        DROP
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)));

    test_case(
        "
        PUSHSLICE x4_

        PUSHSLICE x5_
        PUSHINT 99
        NEWDICT
        PUSHINT 8
        DICTUSET

        PUSHINT 99
        SWAP
        PUSHINT 8
        DICTUREPLACEGET

        XCHG s2
        DROP
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(true)).push(create::slice([0x50])));
}

#[test]
fn test_dict_replacegetref_once_keywriter_to_uint() {
    test_case(
        "
        PUSHSLICE x5_
        NEWC
        STSLICE
        ENDC
        PUSHINT 99
        NEWDICT
        PUSHINT 8
        DICTUREPLACEGETREF

        SWAP
        DROP
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)));

    test_case(
        "
        PUSHSLICE x4_
        NEWC
        STSLICE
        ENDC

        PUSHSLICE x5_
        NEWC
        STSLICE
        ENDC
        PUSHINT 99
        NEWDICT
        PUSHINT 8
        DICTUSETREF

        PUSHINT 99
        SWAP
        PUSHINT 8
        DICTUREPLACEGETREF

        XCHG s2
        DROP
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(true)).push(create::cell([0x50])));
}

#[test]
fn test_dict_add_once() {
    test_case(
        "
        PUSHSLICE x5_
        PUSHSLICE x7_
        NEWDICT
        PUSHINT 3
        DICTADD

        SWAP
        DROP
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(true)));

    test_case(
        "
        PUSHSLICE x4_

        PUSHSLICE x5_
        PUSHSLICE x7_
        NEWDICT
        PUSHINT 3
        DICTSET

        PUSHSLICE x7_
        SWAP
        PUSHINT 3
        DICTADD

        SWAP
        DROP
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)));
}

#[test]
fn test_dict_addref_once() {
    test_case(
        "
        PUSHSLICE x5_
        NEWC
        STSLICE
        ENDC
        PUSHSLICE x7_
        NEWDICT
        PUSHINT 3
        DICTADDREF

        SWAP
        DROP
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(true)));

    test_case(
        "
        PUSHSLICE x4_
        NEWC
        STSLICE
        ENDC

        PUSHSLICE x5_
        NEWC
        STSLICE
        ENDC
        PUSHSLICE x7_
        NEWDICT
        PUSHINT 3
        DICTSETREF

        PUSHSLICE x7_
        SWAP
        PUSHINT 3
        DICTADDREF

        SWAP
        DROP
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)));
}

#[test]
fn test_dict_add_once_keywriter_to_int() {
    test_case(
        "
        PUSHSLICE x5_
        PUSHINT 99
        NEWDICT
        PUSHINT 8
        DICTIADD

        SWAP
        DROP
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(true)));

    test_case(
        "
        PUSHSLICE x4_

        PUSHSLICE x5_
        PUSHINT 99
        NEWDICT
        PUSHINT 8
        DICTISET

        PUSHINT 99
        SWAP
        PUSHINT 8
        DICTIADD

        SWAP
        DROP
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)));
}

#[test]
fn test_dict_addref_once_keywriter_to_int() {
    test_case(
        "
        PUSHSLICE x5_
        NEWC
        STSLICE
        ENDC
        PUSHINT 99
        NEWDICT
        PUSHINT 8
        DICTIADDREF

        SWAP
        DROP
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(true)));

    test_case(
        "
        PUSHSLICE x4_
        NEWC
        STSLICE
        ENDC

        PUSHSLICE x5_
        NEWC
        STSLICE
        ENDC
        PUSHINT 99
        NEWDICT
        PUSHINT 8
        DICTISETREF

        PUSHINT 99
        SWAP
        PUSHINT 8
        DICTIADDREF

        SWAP
        DROP
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)));
}

#[test]
fn test_dict_add_once_keywriter_to_uint() {
    test_case(
        "
        PUSHSLICE x5_
        PUSHINT 99
        NEWDICT
        PUSHINT 8
        DICTUADD

        SWAP
        DROP
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(true)));

    test_case(
        "
        PUSHSLICE x4_

        PUSHSLICE x5_
        PUSHINT 99
        NEWDICT
        PUSHINT 8
        DICTUSET

        PUSHINT 99
        SWAP
        PUSHINT 8
        DICTUADD

        SWAP
        DROP
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)));
}

#[test]
fn test_dict_addref_once_keywriter_to_uint() {
    test_case(
        "
        PUSHSLICE x5_
        NEWC
        STSLICE
        ENDC
        PUSHINT 99
        NEWDICT
        PUSHINT 8
        DICTUADDREF

        SWAP
        DROP
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(true)));

    test_case(
        "
        PUSHSLICE x4_
        NEWC
        STSLICE
        ENDC

        PUSHSLICE x5_
        NEWC
        STSLICE
        ENDC
        PUSHINT 99
        NEWDICT
        PUSHINT 8
        DICTUSETREF

        PUSHINT 99
        SWAP
        PUSHINT 8
        DICTUADDREF

        SWAP
        DROP
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)));
}

#[test]
fn test_dict_addget_once() {
    test_case(
        "
        PUSHSLICE x5_
        PUSHSLICE x7_
        NEWDICT
        PUSHINT 3
        DICTADDGET

        SWAP
        DROP
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(true)));

    test_case(
        "
        PUSHSLICE x4_

        PUSHSLICE x5_
        PUSHSLICE x7_
        NEWDICT
        PUSHINT 3
        DICTSET

        PUSHSLICE x7_
        SWAP
        PUSHINT 3
        DICTADDGET

        XCHG s2
        DROP
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)).push(create::slice([0x50])));
}

#[test]
fn test_dict_addgetref_once() {
    test_case(
        "
        PUSHSLICE x5_
        NEWC
        STSLICE
        ENDC
        PUSHSLICE x7_
        NEWDICT
        PUSHINT 3
        DICTADDGETREF

        SWAP
        DROP
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(true)));

    test_case(
        "
        PUSHSLICE x4_
        NEWC
        STSLICE
        ENDC

        PUSHSLICE x5_
        NEWC
        STSLICE
        ENDC
        PUSHSLICE x7_
        NEWDICT
        PUSHINT 3
        DICTSETREF

        PUSHSLICE x7_
        SWAP
        PUSHINT 3
        DICTADDGETREF

        XCHG s2
        DROP
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)).push(to_cell(vec![0x50])));
}

#[test]
fn test_dict_addget_once_keywriter_to_int() {
    test_case(
        "
        PUSHSLICE x5_
        PUSHINT 99
        NEWDICT
        PUSHINT 8
        DICTIADDGET

        SWAP
        DROP
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(true)));

    test_case(
        "
        PUSHSLICE x4_

        PUSHSLICE x5_
        PUSHINT 99
        NEWDICT
        PUSHINT 8
        DICTISET

        PUSHINT 99
        SWAP
        PUSHINT 8
        DICTIADDGET

        XCHG s2
        DROP
        ",
    )
    .expect_stack(
        Stack::new()
            .push(StackItem::boolean(false))
            .push(StackItem::Slice(SliceData::new(vec![0x50]))),
    );
}

#[test]
fn test_dict_addgetref_once_keywriter_to_int() {
    test_case(
        "
        PUSHSLICE x5_
        NEWC
        STSLICE
        ENDC
        PUSHINT 99
        NEWDICT
        PUSHINT 8
        DICTIADDGETREF

        SWAP
        DROP
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(true)));

    test_case(
        "
        PUSHSLICE x4_
        NEWC
        STSLICE
        ENDC

        PUSHSLICE x5_
        NEWC
        STSLICE
        ENDC
        PUSHINT 99
        NEWDICT
        PUSHINT 8
        DICTISETREF

        PUSHINT 99
        SWAP
        PUSHINT 8
        DICTIADDGETREF

        XCHG s2
        DROP
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)).push(create::cell([0x50])));
}

#[test]
fn test_dict_addget_once_keywriter_to_uint() {
    test_case(
        "
        PUSHSLICE x5_
        PUSHINT 99
        NEWDICT
        PUSHINT 8
        DICTUADDGET

        SWAP
        DROP
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(true)));

    test_case(
        "
        PUSHSLICE x4_

        PUSHSLICE x5_
        PUSHINT 99
        NEWDICT
        PUSHINT 8
        DICTUSET

        PUSHINT 99
        SWAP
        PUSHINT 8
        DICTUADDGET

        XCHG s2
        DROP
        ",
    )
    .expect_stack(
        Stack::new()
            .push(StackItem::boolean(false))
            .push(StackItem::Slice(SliceData::new(vec![0x50]))),
    );
}

#[test]
fn test_dict_addgetref_once_keywriter_to_uint() {
    test_case(
        "
        PUSHSLICE x5_
        NEWC
        STSLICE
        ENDC
        PUSHINT 99
        NEWDICT
        PUSHINT 8
        DICTUADDGETREF

        SWAP
        DROP
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(true)));

    test_case(
        "
        PUSHSLICE x4_
        NEWC
        STSLICE
        ENDC

        PUSHSLICE x5_
        NEWC
        STSLICE
        ENDC
        PUSHINT 99
        NEWDICT
        PUSHINT 8
        DICTUSETREF

        PUSHINT 99
        SWAP
        PUSHINT 8
        DICTUADDGETREF

        XCHG s2
        DROP
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)).push(to_cell([0x50])));
}

#[test]
fn test_dict_get_next_once() {
    test_case(
        "
        PUSHSLICE x5_
        PUSHSLICE xFF80

        PUSHSLICE x4_
        PUSHSLICE x5880
        NEWDICT
        PUSHINT 8
        DICTSET

        PUSHINT 8
        DICTSET

        PUSHSLICE x6080
        SWAP
        PUSHINT 8
        DICTGETNEXT
        ",
    )
    .expect_stack(
        Stack::new()
            .push(StackItem::Slice(SliceData::new(vec![0x50])))
            .push(StackItem::Slice(SliceData::new(vec![0xFF, 0x80])))
            .push(StackItem::boolean(true)),
    );

    test_case(
        "
        PUSHSLICE x6080
        NEWDICT
        PUSHINT 8
        DICTGETNEXT
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)));
}

#[test]
fn test_dict_get_prev_once_run() {
    test_case(
        "
        PUSHSLICE x5_
        PUSHSLICE xFF8_

        PUSHSLICE x4_
        PUSHSLICE x588_
        NEWDICT
        PUSHINT 8
        DICTSET

        PUSHINT 8
        DICTSET

        PUSHSLICE x608_
        SWAP
        PUSHINT 8
        DICTGETPREV
        ",
    )
    .expect_stack(
        Stack::new()
            .push(create::slice([0x40]))
            .push(StackItem::Slice(SliceData::new(vec![0x58, 0x80])))
            .push(StackItem::boolean(true)),
    );

    test_case(
        "
        PUSHSLICE xFF80
        NEWDICT
        PUSHINT 8
        DICTGETPREV
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)));
}

#[test]
fn test_dict_get_nexteq_once() {
    test_case(
        "
        PUSHSLICE x5_
        PUSHSLICE xFF80
        NEWDICT
        PUSHINT 8
        DICTSET

        PUSHSLICE xFF80
        SWAP
        PUSHINT 8
        DICTGETNEXTEQ
        ",
    )
    .expect_stack(
        Stack::new()
            .push(StackItem::Slice(SliceData::new(vec![0x50])))
            .push(StackItem::Slice(SliceData::new(vec![0xFF, 0x80])))
            .push(StackItem::boolean(true)),
    );

    test_case(
        "
        PUSHSLICE x5_
        PUSHSLICE xFF80

        PUSHSLICE x4_
        PUSHSLICE x5880
        NEWDICT
        PUSHINT 8
        DICTSET

        PUSHINT 8
        DICTSET

        PUSHSLICE x6080
        SWAP
        PUSHINT 8
        DICTGETNEXTEQ
        ",
    )
    .expect_stack(
        Stack::new()
            .push(StackItem::Slice(SliceData::new(vec![0x50])))
            .push(StackItem::Slice(SliceData::new(vec![0xFF, 0x80])))
            .push(StackItem::boolean(true)),
    );

    test_case(
        "
        PUSHSLICE xFF80
        NEWDICT
        PUSHINT 8
        DICTGETNEXTEQ
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)));
}

#[test]
fn test_dict_get_preveq_once_keywriter_to_slice() {
    test_case(
        "
        PUSHSLICE x5_
        PUSHSLICE x5880
        NEWDICT
        PUSHINT 8
        DICTSET

        PUSHSLICE x5880
        SWAP
        PUSHINT 8
        DICTGETPREVEQ
        ",
    )
    .expect_stack(
        Stack::new()
            .push(StackItem::Slice(SliceData::new(vec![0x50])))
            .push(StackItem::Slice(SliceData::new(vec![0x58, 0x80])))
            .push(StackItem::boolean(true)),
    );

    test_case(
        "
        PUSHSLICE x5_
        PUSHSLICE xFF80

        PUSHSLICE x4_
        PUSHSLICE x5880
        NEWDICT
        PUSHINT 8
        DICTSET

        PUSHINT 8
        DICTSET

        PUSHSLICE x6080
        SWAP
        PUSHINT 8
        DICTGETPREVEQ
        ",
    )
    .expect_stack(
        Stack::new()
            .push(create::slice([0x40]))
            .push(StackItem::Slice(SliceData::new(vec![0x58, 0x80])))
            .push(StackItem::boolean(true)),
    );

    test_case(
        "
        PUSHSLICE xFF80
        NEWDICT
        PUSHINT 8
        DICTGETPREVEQ
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)));
}

#[test]
fn test_dict_get_next_once_keywriter_to_int() {
    test_case(
        "
        PUSHSLICE x5_
        PUSHINT 80

        PUSHSLICE x7_
        PUSHINT 240
        NEWDICT
        PUSHINT 8
        DICTUSET

        PUSHINT 8
        DICTUSET

        PUSHINT 112
        SWAP
        PUSHINT 8
        DICTUGETNEXT
        ",
    )
    .expect_stack(
        Stack::new()
            .push(StackItem::Slice(SliceData::new(vec![0x70])))
            .push(StackItem::int(240))
            .push(StackItem::boolean(true)),
    );
}

#[test]
fn test_dict_get_next_once_keywriter_to_int_undersize() {
    test_case(
        "
        PUSHSLICE x5_
        PUSHINT 80

        PUSHSLICE x7_
        PUSHINT 240
        NEWDICT
        PUSHINT 8
        DICTUSET

        PUSHINT 8
        DICTUSET

        PUSHNEGPOW2 8
        SWAP
        PUSHINT 8
        DICTIGETNEXT
        ",
    )
    .expect_stack(
        Stack::new()
            .push(StackItem::Slice(SliceData::new(vec![0x70])))
            .push(StackItem::int(-16))
            .push(StackItem::boolean(true)),
    );
}

#[test]
fn test_dict_get_next_once_keywriter_to_int_oversize() {
    test_case(
        "
        PUSHSLICE x5_
        PUSHINT 80

        PUSHSLICE x7_
        PUSHINT 240
        NEWDICT
        PUSHINT 8
        DICTUSET

        PUSHINT 8
        DICTUSET

        PUSHPOW2 8
        SWAP
        PUSHINT 8
        DICTIGETNEXT
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)));
}

#[test]
fn test_dict_get_prev_once_keywriter_to_int() {
    test_case(
        "
        PUSHSLICE x4_
        PUSHINT 80

        PUSHSLICE x7_
        PUSHINT 240
        NEWDICT
        PUSHINT 8
        DICTUSET

        PUSHINT 8
        DICTUSET

        PUSHINT 112
        SWAP
        PUSHINT 8
        DICTUGETPREV
        ",
    )
    .expect_stack(
        Stack::new()
            .push(create::slice([0x40]))
            .push(StackItem::int(80))
            .push(StackItem::boolean(true)),
    );
}

#[test]
fn test_dict_get_prev_once_keywriter_to_int_oversize() {
    test_case(
        "
        PUSHSLICE x4_
        PUSHINT 80

        PUSHSLICE x7_
        PUSHINT 240
        NEWDICT
        PUSHINT 8
        DICTUSET

        PUSHINT 8
        DICTUSET

        PUSHPOW2 8
        SWAP
        PUSHINT 8
        DICTUGETPREV
        ",
    )
    .expect_stack(
        Stack::new()
            .push(create::slice([0x70]))
            .push(StackItem::int(240))
            .push(StackItem::boolean(true)),
    );
}

#[test]
fn test_dict_get_nexteq_once_keywriter_to_int() {
    test_case(
        "
        PUSHSLICE x5_
        PUSHINT -80
        NEWDICT
        PUSHINT 8
        DICTISET

        PUSHINT -80
        SWAP
        PUSHINT 8
        DICTIGETNEXTEQ
        ",
    )
    .expect_stack(
        Stack::new()
            .push(StackItem::Slice(SliceData::new(vec![0x50])))
            .push(StackItem::int(-80))
            .push(StackItem::boolean(true)),
    );

    test_case(
        "
        PUSHSLICE x4_
        PUSHINT 80

        PUSHSLICE x7_
        PUSHINT 240
        NEWDICT
        PUSHINT 8
        DICTUSET

        PUSHINT 8
        DICTUSET

        PUSHINT 112
        SWAP
        PUSHINT 8
        DICTUGETNEXTEQ
        ",
    )
    .expect_stack(
        Stack::new()
            .push(StackItem::Slice(SliceData::new(vec![0x70])))
            .push(StackItem::int(240))
            .push(StackItem::boolean(true)),
    );
}

#[test]
fn test_dict_get_preveq_once_keywriter_to_int() {
    test_case(
        "
        PUSHSLICE x4_
        PUSHINT -80
        NEWDICT
        PUSHINT 8
        DICTISET

        PUSHINT -80
        SWAP
        PUSHINT 8
        DICTIGETPREVEQ
        ",
    )
    .expect_stack(
        Stack::new()
            .push(create::slice([0x40]))
            .push(StackItem::int(-80))
            .push(StackItem::boolean(true)),
    );

    test_case(
        "
        PUSHSLICE x4_
        PUSHINT 80

        PUSHSLICE x7_
        PUSHINT 240
        NEWDICT
        PUSHINT 8
        DICTUSET

        PUSHINT 8
        DICTUSET

        PUSHINT 112
        SWAP
        PUSHINT 8
        DICTUGETPREVEQ
        ",
    )
    .expect_stack(
        Stack::new()
            .push(create::slice([0x40]))
            .push(StackItem::int(80))
            .push(StackItem::boolean(true)),
    );
}

#[test]
fn test_dict_get_next_once_keywriter_to_uint() {
    test_case(
        "
        PUSHSLICE x5_
        PUSHINT 80

        PUSHSLICE x7_
        PUSHINT 240
        NEWDICT
        PUSHINT 8
        DICTUSET

        PUSHINT 8
        DICTUSET

        PUSHINT 112
        SWAP
        PUSHINT 8
        DICTUGETNEXT
        ",
    )
    .expect_stack(
        Stack::new()
            .push(StackItem::Slice(SliceData::new(vec![0x70])))
            .push(StackItem::int(240))
            .push(StackItem::boolean(true)),
    );
}

#[test]
fn test_dict_get_prev_once_keywriter_to_uint() {
    test_case(
        "
        PUSHSLICE x4_
        PUSHINT 80

        PUSHSLICE x7_
        PUSHINT 240
        NEWDICT
        PUSHINT 8
        DICTUSET

        PUSHINT 8
        DICTUSET

        PUSHINT 112
        SWAP
        PUSHINT 8
        DICTUGETPREV
        ",
    )
    .expect_stack(
        Stack::new()
            .push(create::slice([0x40]))
            .push(StackItem::int(80))
            .push(StackItem::boolean(true)),
    );
}

#[test]
fn test_dict_get_nexteq_once_keywriter_to_uint() {
    test_case(
        "
        PUSHSLICE x5_
        PUSHINT 80
        NEWDICT
        PUSHINT 8
        DICTUSET

        PUSHINT 80
        SWAP
        PUSHINT 8
        DICTUGETNEXTEQ
        ",
    )
    .expect_stack(
        Stack::new()
            .push(StackItem::Slice(SliceData::new(vec![0x50])))
            .push(StackItem::int(80))
            .push(StackItem::boolean(true)),
    );

    test_case(
        "
        PUSHSLICE x4_
        PUSHINT 80

        PUSHSLICE x7_
        PUSHINT 240
        NEWDICT
        PUSHINT 8
        DICTUSET

        PUSHINT 8
        DICTUSET

        PUSHINT 112
        SWAP
        PUSHINT 8
        DICTUGETNEXTEQ
        ",
    )
    .expect_stack(
        Stack::new()
            .push(StackItem::Slice(SliceData::new(vec![0x70])))
            .push(StackItem::int(240))
            .push(StackItem::boolean(true)),
    );
}

#[test]
fn test_dict_get_preveq_once_keywriter_to_uint() {
    test_case(
        "
        PUSHSLICE x4_
        PUSHINT 80
        NEWDICT
        PUSHINT 8
        DICTUSET

        PUSHINT 80
        SWAP
        PUSHINT 8
        DICTUGETPREVEQ
        ",
    )
    .expect_stack(
        Stack::new()
            .push(create::slice([0x40]))
            .push(StackItem::int(80))
            .push(StackItem::boolean(true)),
    );

    test_case(
        "
        PUSHSLICE x4_
        PUSHINT 80

        PUSHSLICE x7_
        PUSHINT 240
        NEWDICT
        PUSHINT 8
        DICTUSET

        PUSHINT 8
        DICTUSET

        PUSHINT 112
        SWAP
        PUSHINT 8
        DICTUGETPREVEQ
        ",
    )
    .expect_stack(
        Stack::new()
            .push(create::slice([0x40]))
            .push(StackItem::int(80))
            .push(StackItem::boolean(true)),
    );
}

#[test]
fn test_delete_on_empty_dict() {
    test_case(
        "
        PUSHSLICE x5_
        NEWDICT
        PUSHINT 3
        DICTDEL
        SWAP
        DICTEMPTY
    ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)).push(StackItem::boolean(true)));

    test_case(
        "
        PUSHINT 100
        NEWDICT
        PUSHINT 8
        DICTUDEL
        SWAP
        DICTEMPTY
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)).push(StackItem::boolean(true)));

    test_case(
        "
        PUSHINT 100
        NEWDICT
        PUSHINT 8
        DICTIDEL
        SWAP
        DICTEMPTY
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)).push(StackItem::boolean(true)));

    test_case(
        "
        PUSHSLICE x5_
        NEWDICT
        PUSHINT 3
        DICTDELGET
        SWAP
        DICTEMPTY
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)).push(StackItem::boolean(true)));

    test_case(
        "
        PUSHSLICE x5_
        NEWDICT
        PUSHINT 3
        DICTDELGETREF
        SWAP
        DICTEMPTY
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)).push(StackItem::boolean(true)));

    test_case(
        "
        PUSHINT 120
        NEWDICT
        PUSHINT 8
        DICTUDELGET
        SWAP
        DICTEMPTY
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)).push(StackItem::boolean(true)));

    test_case(
        "
        PUSHINT 120
        NEWDICT
        PUSHINT 8
        DICTUDELGETREF
        SWAP
        DICTEMPTY
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)).push(StackItem::boolean(true)));

    test_case(
        "
        PUSHINT 120
        NEWDICT
        PUSHINT 8
        DICTIDELGET
        SWAP
        DICTEMPTY
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)).push(StackItem::boolean(true)));

    test_case(
        "
        PUSHINT 120
        NEWDICT
        PUSHINT 8
        DICTIDELGETREF
        SWAP
        DICTEMPTY
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)).push(StackItem::boolean(true)));
}

#[test]
fn test_delete_on_not_empty_dict() {
    test_case(
        "
        PUSHSLICE x5_
        PUSHSLICE x7_
        NEWDICT
        PUSHINT 3
        DICTSET

        PUSHSLICE x7_
        SWAP
        PUSHINT 3
        DICTDEL

        PUSHCONT {
            PUSHSLICE x7_
            SWAP
            PUSHINT 3
            DICTGET
        }
        IF
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)));

    test_case(
        "
        PUSHSLICE x5_
        PUSHINT 120
        NEWDICT
        PUSHINT 8
        DICTUSET

        PUSHINT 120
        SWAP
        PUSHINT 8
        DICTUDEL

        PUSHCONT {
            PUSHINT 120
            SWAP
            PUSHINT 8
            DICTUGET
        }
        IF
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)));

    test_case(
        "
        PUSHSLICE x5_
        PUSHINT -120
        NEWDICT
        PUSHINT 8
        DICTISET

        PUSHINT -120
        SWAP
        PUSHINT 8
        DICTIDEL

        PUSHCONT {
            PUSHINT -120
            SWAP
            PUSHINT 8
            DICTIGET
        }
        IF
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)));

    test_case(
        "
        PUSHSLICE x5_
        PUSHSLICE x7_
        NEWDICT
        PUSHINT 3
        DICTSET

        PUSHSLICE x7_
        SWAP
        PUSHINT 3
        DICTDELGET

        PUSHCONT {
            SWAP
            PUSHSLICE x7_
            SWAP
            PUSHINT 3
            DICTGET
        }
        IF
        ",
    )
    .expect_stack(
        Stack::new()
            .push(StackItem::Slice(SliceData::new(vec![0x50])))
            .push(StackItem::boolean(false)),
    );

    test_case(
        "
        PUSHSLICE x5_
        PUSHINT 100
        NEWDICT
        PUSHINT 8
        DICTUSET

        PUSHINT 100
        SWAP
        PUSHINT 8
        DICTUDELGET

        PUSHCONT {
            SWAP
            PUSHINT 100
            SWAP
            PUSHINT 8
            DICTUGET
        }
        IF
        ",
    )
    .expect_stack(
        Stack::new()
            .push(StackItem::Slice(SliceData::new(vec![0x50])))
            .push(StackItem::boolean(false)),
    );

    test_case(
        "
        PUSHSLICE x5_
        PUSHINT -100
        NEWDICT
        PUSHINT 8
        DICTISET

        PUSHINT -100
        SWAP
        PUSHINT 8
        DICTIDELGET

        PUSHCONT {
            SWAP
            PUSHINT -100
            SWAP
            PUSHINT 8
            DICTIGET
        }
        IF
        ",
    )
    .expect_stack(
        Stack::new()
            .push(StackItem::Slice(SliceData::new(vec![0x50])))
            .push(StackItem::boolean(false)),
    );

    test_case(
        "
        PUSHSLICE x5_
        NEWC
        STSLICE
        ENDC
        PUSHSLICE x7_
        NEWDICT
        PUSHINT 3
        DICTSETREF

        PUSHSLICE x7_
        SWAP
        PUSHINT 3
        DICTDELGETREF

        PUSHCONT {
            SWAP
            PUSHSLICE x7_
            SWAP
            PUSHINT 3
            DICTGET
        }
        IF
        ",
    )
    .expect_stack(Stack::new().push(to_cell([0x50])).push(StackItem::boolean(false)));

    test_case(
        "
        PUSHSLICE x5_
        NEWC
        STSLICE
        ENDC
        PUSHINT 133
        NEWDICT
        PUSHINT 8
        DICTUSETREF

        PUSHINT 133
        SWAP
        PUSHINT 8
        DICTUDELGETREF

        PUSHCONT {
            SWAP
            PUSHINT 133
            SWAP
            PUSHINT 8
            DICTUGET
        }
        IF
        ",
    )
    .expect_stack(Stack::new().push(to_cell([0x50])).push(StackItem::boolean(false)));

    test_case(
        "
        PUSHSLICE x5_
        NEWC
        STSLICE
        ENDC
        PUSHINT 123
        NEWDICT
        PUSHINT 8
        DICTISETREF

        PUSHINT 123
        SWAP
        PUSHINT 8
        DICTIDELGETREF

        PUSHCONT {
            SWAP
            PUSHINT 123
            SWAP
            PUSHINT 8
            DICTIGET
        }
        IF
        ",
    )
    .expect_stack(Stack::new().push(to_cell([0x50])).push(StackItem::boolean(false)));
}

#[test]
fn test_dict_min_once() {
    test_case(
        "
        PUSHSLICE x5_
        PUSHSLICE xFF80

        PUSHSLICE x4_
        PUSHSLICE x5880
        NEWDICT
        PUSHINT 8
        DICTSET

        PUSHINT 8
        DICTSET

        PUSHINT 8
        DICTMIN
        ",
    )
    .expect_stack(
        Stack::new()
            .push(create::slice([0x40]))
            .push(StackItem::Slice(SliceData::new(vec![0x58, 0x80])))
            .push(StackItem::boolean(true)),
    );

    test_case(
        "
        NEWDICT
        PUSHINT 8
        DICTMIN
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)));
}

#[test]
fn test_dict_minref_once() {
    test_case(
        "
        PUSHSLICE x5_
        NEWC
        STSLICE
        ENDC
        PUSHSLICE xFF80

        PUSHSLICE x4_
        NEWC
        STSLICE
        ENDC
        PUSHSLICE x5880
        NEWDICT
        PUSHINT 8
        DICTSETREF

        PUSHINT 8
        DICTSETREF

        PUSHINT 8
        DICTMINREF
        ",
    )
    .expect_stack(
        Stack::new()
            .push(to_cell([0x40]))
            .push(StackItem::Slice(SliceData::new(vec![0x58, 0x80])))
            .push(StackItem::boolean(true)),
    );

    test_case(
        "
        NEWDICT
        PUSHINT 8
        DICTMINREF
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)));
}

#[test]
fn test_dict_umin_once() {
    test_case(
        "
        PUSHSLICE x5_
        PUSHSLICE xFF80

        PUSHSLICE x4_
        PUSHSLICE x5880
        NEWDICT
        PUSHINT 8
        DICTSET

        PUSHINT 8
        DICTSET

        PUSHINT 8
        DICTUMIN
        ",
    )
    .expect_stack(
        Stack::new()
            .push(create::slice([0x40]))
            .push(StackItem::int(88)) // DEC(88) == HEX(58)
            .push(StackItem::boolean(true)),
    );

    test_case(
        "
        NEWDICT
        PUSHINT 8
        DICTUMIN
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)));
}

#[test]
fn test_dict_imin_once() {
    test_case(
        "
        PUSHSLICE x5_
        PUSHSLICE xFF80

        PUSHSLICE x4_
        PUSHSLICE x5880
        NEWDICT
        PUSHINT 8
        DICTSET

        PUSHINT 8
        DICTSET

        PUSHINT 8
        DICTIMIN
        ",
    )
    .expect_stack(
        Stack::new()
            .push(StackItem::Slice(SliceData::new(vec![0x50])))
            .push(StackItem::int(-1))
            .push(StackItem::boolean(true)),
    );

    test_case(
        "
        NEWDICT
        PUSHINT 8
        DICTIMIN
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)));
}

#[test]
fn test_dict_uminref_once() {
    test_case(
        "
        PUSHSLICE x5_
        NEWC
        STSLICE
        ENDC
        PUSHSLICE xFF80

        PUSHSLICE x4_
        NEWC
        STSLICE
        ENDC
        PUSHSLICE x5880
        NEWDICT
        PUSHINT 8
        DICTSETREF

        PUSHINT 8
        DICTSETREF

        PUSHINT 8
        DICTUMINREF
        ",
    )
    .expect_stack(
        Stack::new()
            .push(to_cell(vec![0x40]))
            .push(StackItem::int(88)) // DEC(88) == HEX(58)
            .push(StackItem::boolean(true)),
    );

    test_case(
        "
        NEWDICT
        PUSHINT 8
        DICTUMINREF
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)));
}

#[test]
fn test_dict_iminref_once() {
    test_case(
        "
        PUSHSLICE x5_
        NEWC
        STSLICE
        ENDC
        PUSHSLICE xFF80

        PUSHSLICE x4_
        NEWC
        STSLICE
        ENDC
        PUSHSLICE x5880
        NEWDICT
        PUSHINT 8
        DICTSETREF

        PUSHINT 8
        DICTSETREF

        PUSHINT 8
        DICTIMINREF
        ",
    )
    .expect_stack(
        Stack::new()
            .push(to_cell(vec![0x50]))
            .push(StackItem::int(-1))
            .push(StackItem::boolean(true)),
    );

    test_case(
        "
        NEWDICT
        PUSHINT 8
        DICTIMINREF
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)));
}

#[test]
fn test_dict_max_once() {
    test_case(
        "
        PUSHSLICE x5_
        PUSHSLICE xFF80

        PUSHSLICE x4_
        PUSHSLICE x5880
        NEWDICT
        PUSHINT 8
        DICTSET

        PUSHINT 8
        DICTSET

        PUSHINT 8
        DICTMAX
        ",
    )
    .expect_stack(
        Stack::new()
            .push(StackItem::Slice(SliceData::new(vec![0x50])))
            .push(StackItem::Slice(SliceData::new(vec![0xFF, 0x80])))
            .push(StackItem::boolean(true)),
    );

    test_case(
        "
        NEWDICT
        PUSHINT 8
        DICTMAX
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)));
}

#[test]
fn test_dict_maxref_once() {
    test_case(
        "
        PUSHSLICE x5_
        NEWC
        STSLICE
        ENDC
        PUSHSLICE xFF80

        PUSHSLICE x4_
        NEWC
        STSLICE
        ENDC
        PUSHSLICE x5880
        NEWDICT
        PUSHINT 8
        DICTSETREF

        PUSHINT 8
        DICTSETREF

        PUSHINT 8
        DICTMAXREF
        ",
    )
    .expect_stack(
        Stack::new()
            .push(to_cell([0x50]))
            .push(StackItem::Slice(SliceData::new(vec![0xFF, 0x80])))
            .push(StackItem::boolean(true)),
    );

    test_case(
        "
        NEWDICT
        PUSHINT 8
        DICTMAXREF
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)));
}

#[test]
fn test_dict_umax_once() {
    test_case(
        "
        PUSHSLICE x5_
        PUSHSLICE xFF80

        PUSHSLICE x4_
        PUSHSLICE x5880
        NEWDICT
        PUSHINT 8
        DICTSET

        PUSHINT 8
        DICTSET

        PUSHINT 8
        DICTUMAX
        ",
    )
    .expect_stack(
        Stack::new()
            .push(StackItem::Slice(SliceData::new(vec![0x50])))
            .push(StackItem::int(255)) // DEC(255) == HEX(FF)
            .push(StackItem::boolean(true)),
    );

    test_case(
        "
        NEWDICT
        PUSHINT 8
        DICTUMAX
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)));
}

#[test]
fn test_dict_imax_once() {
    test_case(
        "
        PUSHSLICE x5_
        PUSHSLICE xFF80

        PUSHSLICE x4_
        PUSHSLICE x5880
        NEWDICT
        PUSHINT 8
        DICTSET

        PUSHINT 8
        DICTSET

        PUSHINT 8
        DICTIMAX
        ",
    )
    .expect_stack(
        Stack::new()
            .push(create::slice([0x40]))
            .push(StackItem::int(0x58))
            .push(StackItem::boolean(true)),
    );

    test_case(
        "
        NEWDICT
        PUSHINT 8
        DICTIMAX
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)));
}

#[test]
fn test_dict_umaxref_once() {
    test_case(
        "
        PUSHSLICE x5_
        NEWC
        STSLICE
        ENDC
        PUSHSLICE xFF80

        PUSHSLICE x4_
        NEWC
        STSLICE
        ENDC
        PUSHSLICE x5880
        NEWDICT
        PUSHINT 8
        DICTSETREF

        PUSHINT 8
        DICTSETREF

        PUSHINT 8
        DICTUMAXREF
        ",
    )
    .expect_stack(
        Stack::new()
            .push(to_cell([0x50]))
            .push(StackItem::int(255)) // DEC(255) == HEX(FF)
            .push(StackItem::boolean(true)),
    );

    test_case(
        "
        NEWDICT
        PUSHINT 8
        DICTUMAXREF
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)));
}

#[test]
fn test_dict_imaxref_once() {
    test_case(
        "
        PUSHSLICE x5_
        NEWC
        STSLICE
        ENDC
        PUSHSLICE xFF80

        PUSHSLICE x4_
        NEWC
        STSLICE
        ENDC
        PUSHSLICE x5880
        NEWDICT
        PUSHINT 8
        DICTSETREF

        PUSHINT 8
        DICTSETREF

        PUSHINT 8
        DICTIMAXREF
        ",
    )
    .expect_stack(
        Stack::new()
            .push(to_cell([0x40]))
            .push(StackItem::int(0x58))
            .push(StackItem::boolean(true)),
    );

    test_case(
        "
        NEWDICT
        PUSHINT 8
        DICTIMAXREF
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)));
}

#[test]
fn test_dict_remmin_once() {
    test_case(
        "
        PUSHSLICE x5_
        PUSHSLICE xFF80

        PUSHSLICE x4_
        PUSHSLICE x5880
        NEWDICT
        PUSHINT 8
        DICTSET

        PUSHINT 8
        DICTSET

        PUSHINT 8
        DICTREMMIN

        XCHG s3
        PUSHSLICE x5880
        SWAP
        PUSHINT 8
        DICTGET
        ",
    )
    .expect_stack(
        Stack::new()
            .push(StackItem::boolean(true))
            .push(create::slice([0x40]))
            .push(StackItem::Slice(SliceData::new(vec![0x58, 0x80])))
            .push(StackItem::boolean(false)),
    );

    test_case(
        "
        NEWDICT
        PUSHINT 8
        DICTREMMIN
        ",
    )
    .expect_stack(Stack::new().push(StackItem::default()).push(StackItem::boolean(false)));
}

#[test]
fn test_dict_remminref_once() {
    test_case(
        "
        PUSHSLICE x5_
        NEWC
        STSLICE
        ENDC
        PUSHSLICE xFF80

        PUSHSLICE x4_
        NEWC
        STSLICE
        ENDC
        PUSHSLICE x5880
        NEWDICT
        PUSHINT 8
        DICTSETREF

        PUSHINT 8
        DICTSETREF

        PUSHINT 8
        DICTREMMINREF

        XCHG s3
        PUSHSLICE x5880
        SWAP
        PUSHINT 8
        DICTGETREF
        ",
    )
    .expect_stack(
        Stack::new()
            .push(StackItem::boolean(true))
            .push(to_cell([0x40]))
            .push(StackItem::Slice(SliceData::new(vec![0x58, 0x80])))
            .push(StackItem::boolean(false)),
    );

    test_case(
        "
        NEWDICT
        PUSHINT 8
        DICTREMMINREF
        ",
    )
    .expect_stack(Stack::new().push(StackItem::default()).push(StackItem::boolean(false)));
}

#[test]
fn test_dict_uremmin_once() {
    test_case(
        "
        PUSHSLICE x5_
        PUSHSLICE xFF80

        PUSHSLICE x4_
        PUSHSLICE x5880
        NEWDICT
        PUSHINT 8
        DICTSET

        PUSHINT 8
        DICTSET

        PUSHINT 8
        DICTUREMMIN

        XCHG s3
        PUSHSLICE x5880
        SWAP
        PUSHINT 8
        DICTGET
        ",
    )
    .expect_stack(
        Stack::new()
            .push(StackItem::boolean(true))
            .push(create::slice([0x40]))
            .push(StackItem::int(88)) // DEC(88) == HEX(58)
            .push(StackItem::boolean(false)),
    );

    test_case(
        "
        NEWDICT
        PUSHINT 8
        DICTUREMMIN
        ",
    )
    .expect_stack(Stack::new().push(StackItem::default()).push(StackItem::boolean(false)));
}

#[test]
fn test_dict_uremminref_once() {
    test_case(
        "
        PUSHSLICE x5_
        NEWC
        STSLICE
        ENDC
        PUSHSLICE xFF80

        PUSHSLICE x4_
        NEWC
        STSLICE
        ENDC
        PUSHSLICE x5880
        NEWDICT
        PUSHINT 8
        DICTSETREF

        PUSHINT 8
        DICTSETREF

        PUSHINT 8
        DICTUREMMINREF

        XCHG s3
        PUSHSLICE x5880
        SWAP
        PUSHINT 8
        DICTGETREF
        ",
    )
    .expect_stack(
        Stack::new()
            .push(StackItem::boolean(true))
            .push(to_cell([0x40]))
            .push(StackItem::int(88)) // DEC(88) == HEX(58)
            .push(StackItem::boolean(false)),
    );

    test_case(
        "
        NEWDICT
        PUSHINT 8
        DICTUREMMINREF
        ",
    )
    .expect_stack(Stack::new().push(StackItem::default()).push(StackItem::boolean(false)));
}

#[test]
fn test_dict_iremmin_once() {
    test_case(
        "
        PUSHSLICE x5_
        PUSHSLICE xFF80

        PUSHSLICE x4_
        PUSHSLICE x5880
        NEWDICT
        PUSHINT 8
        DICTSET

        PUSHINT 8
        DICTSET

        PUSHINT 8
        DICTIREMMIN

        XCHG s3
        PUSHSLICE xFF80
        SWAP
        PUSHINT 8
        DICTGET
        ",
    )
    .expect_stack(
        Stack::new()
            .push(StackItem::boolean(true))
            .push(StackItem::Slice(SliceData::new(vec![0x50])))
            .push(StackItem::int(-1))
            .push(StackItem::boolean(false)),
    );

    test_case(
        "
        NEWDICT
        PUSHINT 8
        DICTIREMMIN
        ",
    )
    .expect_stack(Stack::new().push(StackItem::default()).push(StackItem::boolean(false)));
}

#[test]
fn test_dict_iremminref_once() {
    test_case(
        "
        PUSHSLICE x5_
        NEWC
        STSLICE
        ENDC
        PUSHSLICE xFF80

        PUSHSLICE x4_
        NEWC
        STSLICE
        ENDC
        PUSHSLICE x5880
        NEWDICT
        PUSHINT 8
        DICTSETREF

        PUSHINT 8
        DICTSETREF

        PUSHINT 8
        DICTIREMMINREF

        XCHG s3
        PUSHSLICE xFF80
        SWAP
        PUSHINT 8
        DICTGETREF
        ",
    )
    .expect_stack(
        Stack::new()
            .push(StackItem::boolean(true))
            .push(to_cell([0x50]))
            .push(StackItem::int(-1))
            .push(StackItem::boolean(false)),
    );

    test_case(
        "
        NEWDICT
        PUSHINT 8
        DICTIREMMINREF
        ",
    )
    .expect_stack(Stack::new().push(StackItem::default()).push(StackItem::boolean(false)));
}

#[test]
fn test_dict_remmax_once() {
    test_case(
        "
        PUSHSLICE x5_
        PUSHSLICE xFF80

        PUSHSLICE x4_
        PUSHSLICE x5880
        NEWDICT
        PUSHINT 8
        DICTSET

        PUSHINT 8
        DICTSET

        PUSHINT 8
        DICTREMMAX

        XCHG s3
        PUSHSLICE xFF80
        SWAP
        PUSHINT 8
        DICTGET
        ",
    )
    .expect_stack(
        Stack::new()
            .push(StackItem::boolean(true))
            .push(StackItem::Slice(SliceData::new(vec![0x50])))
            .push(StackItem::Slice(SliceData::new(vec![0xFF, 0x80])))
            .push(StackItem::boolean(false)),
    );

    test_case(
        "
        NEWDICT
        PUSHINT 8
        DICTREMMAX
        ",
    )
    .expect_stack(Stack::new().push(StackItem::default()).push(StackItem::boolean(false)));
}

#[test]
fn test_dict_remmaxref_once() {
    test_case(
        "
        PUSHSLICE x5_
        NEWC
        STSLICE
        ENDC
        PUSHSLICE xFF80

        PUSHSLICE x4_
        NEWC
        STSLICE
        ENDC
        PUSHSLICE x5880
        NEWDICT
        PUSHINT 8
        DICTSETREF

        PUSHINT 8
        DICTSETREF

        PUSHINT 8
        DICTREMMAXREF

        XCHG s3
        PUSHSLICE xFF80
        SWAP
        PUSHINT 8
        DICTGETREF
        ",
    )
    .expect_stack(
        Stack::new()
            .push(StackItem::boolean(true))
            .push(to_cell([0x50]))
            .push(StackItem::Slice(SliceData::new(vec![0xFF, 0x80])))
            .push(StackItem::boolean(false)),
    );

    test_case(
        "
        NEWDICT
        PUSHINT 8
        DICTREMMAXREF
        ",
    )
    .expect_stack(Stack::new().push(StackItem::default()).push(StackItem::boolean(false)));
}

#[test]
fn test_dict_uremmax_once() {
    test_case(
        "
        PUSHSLICE x5_
        PUSHSLICE xFF80

        PUSHSLICE x4_
        PUSHSLICE x5880
        NEWDICT
        PUSHINT 8
        DICTSET

        PUSHINT 8
        DICTSET

        PUSHINT 8
        DICTUREMMAX

        XCHG s3
        PUSHSLICE xFF80
        SWAP
        PUSHINT 8
        DICTGET
        ",
    )
    .expect_stack(
        Stack::new()
            .push(StackItem::boolean(true))
            .push(StackItem::Slice(SliceData::new(vec![0x50])))
            .push(StackItem::int(255)) // DEC(255) == HEX(FF)
            .push(StackItem::boolean(false)),
    );

    test_case(
        "
        NEWDICT
        PUSHINT 8
        DICTUREMMAX
        ",
    )
    .expect_stack(Stack::new().push(StackItem::default()).push(StackItem::boolean(false)));
}

#[test]
fn test_dict_uremmmaxref_once() {
    test_case(
        "
        PUSHSLICE x5_
        NEWC
        STSLICE
        ENDC
        PUSHSLICE xFF80

        PUSHSLICE x4_
        NEWC
        STSLICE
        ENDC
        PUSHSLICE x5880
        NEWDICT
        PUSHINT 8
        DICTSETREF

        PUSHINT 8
        DICTSETREF

        PUSHINT 8
        DICTUREMMAXREF

        XCHG s3
        PUSHSLICE xFF80
        SWAP
        PUSHINT 8
        DICTGETREF
        ",
    )
    .expect_stack(
        Stack::new()
            .push(StackItem::boolean(true))
            .push(to_cell([0x50]))
            .push(StackItem::int(255)) // DEC(255) == HEX(FF)
            .push(StackItem::boolean(false)),
    );

    test_case(
        "
        NEWDICT
        PUSHINT 8
        DICTUREMMAXREF
        ",
    )
    .expect_stack(Stack::new().push(StackItem::default()).push(StackItem::boolean(false)));
}

#[test]
fn test_dict_iremmax_once() {
    test_case(
        "
        PUSHSLICE x5_
        PUSHSLICE xFF80

        PUSHSLICE x4_
        PUSHSLICE x5880
        NEWDICT
        PUSHINT 8
        DICTSET

        PUSHINT 8
        DICTSET

        PUSHINT 8
        DICTIREMMAX

        XCHG s3
        PUSHSLICE x5880
        SWAP
        PUSHINT 8
        DICTGET
        ",
    )
    .expect_stack(
        Stack::new()
            .push(StackItem::boolean(true))
            .push(create::slice([0x40]))
            .push(StackItem::int(0x58))
            .push(StackItem::boolean(false)),
    );

    test_case(
        "
        NEWDICT
        PUSHINT 8
        DICTIREMMAX
        ",
    )
    .expect_stack(Stack::new().push(StackItem::default()).push(StackItem::boolean(false)));
}

#[test]
fn test_dict_iremmmaxref_once() {
    test_case(
        "
        PUSHSLICE x5_
        NEWC
        STSLICE
        ENDC
        PUSHSLICE xFF80

        PUSHSLICE x4_
        NEWC
        STSLICE
        ENDC
        PUSHSLICE x5880
        NEWDICT
        PUSHINT 8
        DICTSETREF

        PUSHINT 8
        DICTSETREF

        PUSHINT 8
        DICTIREMMAXREF

        XCHG s3
        PUSHSLICE x5880
        SWAP
        PUSHINT 8
        DICTGETREF
        ",
    )
    .expect_stack(
        Stack::new()
            .push(StackItem::boolean(true))
            .push(to_cell([0x40]))
            .push(StackItem::int(0x58))
            .push(StackItem::boolean(false)),
    );

    test_case(
        "
        NEWDICT
        PUSHINT 8
        DICTIREMMAXREF
        ",
    )
    .expect_stack(Stack::new().push(StackItem::default()).push(StackItem::boolean(false)));
}

#[test]
fn test_dict_setb_and_get_once() {
    test_case(
        "
        PUSHSLICE x5_
        NEWC
        STSLICE

        PUSHSLICE x7_
        NEWDICT
        PUSHINT 3
        DICTSETB

        PUSHSLICE x7_
        SWAP
        PUSHINT 3
        DICTGET
        ",
    )
    .expect_stack(
        Stack::new()
            .push(StackItem::Slice(SliceData::new(vec![0x50])))
            .push(StackItem::boolean(true)),
    );
}

#[test]
fn test_dict_setb_keywriter_to_uint_and_uget_once() {
    test_case(
        "
        PUSHSLICE x5_
        NEWC
        STSLICE

        PUSHINT 120
        NEWDICT
        PUSHINT 8
        DICTUSETB

        PUSHINT 120
        SWAP
        PUSHINT 8
        DICTUGET
        ",
    )
    .expect_stack(
        Stack::new()
            .push(StackItem::Slice(SliceData::new(vec![0x50])))
            .push(StackItem::boolean(true)),
    );
}

#[test]
fn test_dict_setb_keywriter_to_int_and_get_once() {
    test_case(
        "
        PUSHSLICE x5_
        NEWC
        STSLICE

        PUSHINT 120
        NEWDICT
        PUSHINT 8
        DICTISETB

        PUSHINT 120
        SWAP
        PUSHINT 8
        DICTIGET
        ",
    )
    .expect_stack(
        Stack::new()
            .push(StackItem::Slice(SliceData::new(vec![0x50])))
            .push(StackItem::boolean(true)),
    );
}

#[test]
fn test_dict_setb_and_setgetb_once() {
    test_case(
        "
        PUSHSLICE x5_
        NEWC
        STSLICE

        PUSHSLICE x7_
        NEWDICT
        PUSHINT 3
        DICTSETB

        PUSHSLICE x7_
        NEWC
        STSLICE

        SWAP
        PUSHSLICE x7_
        SWAP
        PUSHINT 3
        DICTSETGETB

        XCHG s2
        PUSHSLICE x7_
        SWAP
        PUSHINT 3
        DICTGET
        ",
    )
    .expect_stack(
        Stack::new()
            .push(StackItem::boolean(true))
            .push(StackItem::Slice(SliceData::new(vec![0x50])))
            .push(StackItem::Slice(SliceData::new(vec![0x70])))
            .push(StackItem::boolean(true)),
    );
}

#[test]
fn test_dict_setb_keywriter_to_uint_and_usetgetb_once() {
    test_case(
        "
        PUSHSLICE x5_
        NEWC
        STSLICE

        PUSHINT 125
        NEWDICT
        PUSHINT 8
        DICTUSETB

        PUSHSLICE x7_
        NEWC
        STSLICE

        SWAP
        PUSHINT 125
        SWAP
        PUSHINT 8
        DICTUSETGETB

        XCHG s2
        PUSHINT 125
        SWAP
        PUSHINT 8
        DICTUGET
        ",
    )
    .expect_stack(
        Stack::new()
            .push(StackItem::boolean(true))
            .push(StackItem::Slice(SliceData::new(vec![0x50])))
            .push(StackItem::Slice(SliceData::new(vec![0x70])))
            .push(StackItem::boolean(true)),
    );
}

#[test]
fn test_dict_setb_keywriter_to_int_and_setgetb_once() {
    test_case(
        "
        PUSHSLICE x5_
        NEWC
        STSLICE

        PUSHINT 125
        NEWDICT
        PUSHINT 8
        DICTISETB

        PUSHSLICE x7_
        NEWC
        STSLICE

        SWAP
        PUSHINT 125
        SWAP
        PUSHINT 8
        DICTISETGETB

        XCHG s2
        PUSHINT 125
        SWAP
        PUSHINT 8
        DICTIGET
        ",
    )
    .expect_stack(
        Stack::new()
            .push(StackItem::boolean(true))
            .push(StackItem::Slice(SliceData::new(vec![0x50])))
            .push(StackItem::Slice(SliceData::new(vec![0x70])))
            .push(StackItem::boolean(true)),
    );
}

#[test]
fn test_dict_replaceb_once() {
    test_case(
        "
        PUSHSLICE x5_
        NEWC
        STSLICE

        PUSHSLICE x7_
        NEWDICT
        PUSHINT 3
        DICTREPLACEB

        SWAP
        DROP
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)));

    test_case(
        "
        PUSHSLICE x4_
        NEWC
        STSLICE

        PUSHSLICE x5_
        NEWC
        STSLICE
        PUSHSLICE x7_
        NEWDICT
        PUSHINT 3
        DICTSETB

        PUSHSLICE x7_
        SWAP
        PUSHINT 3
        DICTREPLACEB

        SWAP
        DROP
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(true)));
}

#[test]
fn test_dict_replaceb_once_keywriter_to_uint() {
    test_case(
        "
        PUSHSLICE x5_
        NEWC
        STSLICE
        PUSHINT 99
        NEWDICT
        PUSHINT 8
        DICTUREPLACEB

        SWAP
        DROP
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)));

    test_case(
        "
        PUSHSLICE x4_
        NEWC
        STSLICE

        PUSHSLICE x5_
        NEWC
        STSLICE
        PUSHINT 99
        NEWDICT
        PUSHINT 8
        DICTUSETB

        PUSHINT 99
        SWAP
        PUSHINT 8
        DICTUREPLACEB

        SWAP
        DROP
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(true)));
}

#[test]
fn test_dict_replaceb_once_keywriter_to_int() {
    test_case(
        "
        PUSHSLICE x5_
        NEWC
        STSLICE
        PUSHINT 99
        NEWDICT
        PUSHINT 8
        DICTIREPLACEB

        SWAP
        DROP
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)));

    test_case(
        "
        PUSHSLICE x4_
        NEWC
        STSLICE

        PUSHSLICE x5_
        NEWC
        STSLICE
        PUSHINT 99
        NEWDICT
        PUSHINT 8
        DICTISETB

        PUSHINT 99
        SWAP
        PUSHINT 8
        DICTIREPLACEB

        SWAP
        DROP
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(true)));
}

#[test]
fn test_dict_replacegetb_once() {
    test_case(
        "
        PUSHSLICE x5_
        NEWC
        STSLICE
        PUSHSLICE x7_
        NEWDICT
        PUSHINT 3
        DICTREPLACEGETB

        SWAP
        DROP
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)));

    test_case(
        "
        PUSHSLICE x4_
        NEWC
        STSLICE

        PUSHSLICE x5_
        NEWC
        STSLICE
        PUSHSLICE x7_
        NEWDICT
        PUSHINT 3
        DICTSETB

        PUSHSLICE x7_
        SWAP
        PUSHINT 3
        DICTREPLACEGETB

        XCHG s2
        DROP
        ",
    )
    .expect_stack(
        Stack::new()
            .push(StackItem::boolean(true))
            .push(StackItem::Slice(SliceData::new(vec![0x50]))),
    );
}

#[test]
fn test_dict_replacegetb_once_keywriter_to_uint() {
    test_case(
        "
        PUSHSLICE x5_
        NEWC
        STSLICE
        PUSHINT 99
        NEWDICT
        PUSHINT 8
        DICTUREPLACEGETB

        SWAP
        DROP
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)));

    test_case(
        "
        PUSHSLICE x4_
        NEWC
        STSLICE

        PUSHSLICE x5_
        NEWC
        STSLICE
        PUSHINT 99
        NEWDICT
        PUSHINT 8
        DICTUSETB

        PUSHINT 99
        SWAP
        PUSHINT 8
        DICTUREPLACEGETB

        XCHG s2
        DROP
        ",
    )
    .expect_stack(
        Stack::new()
            .push(StackItem::boolean(true))
            .push(StackItem::Slice(SliceData::new(vec![0x50]))),
    );
}

#[test]
fn test_dict_replacegetb_once_keywriter_to_int() {
    test_case(
        "
        PUSHSLICE x5_
        NEWC
        STSLICE
        PUSHINT 99
        NEWDICT
        PUSHINT 8
        DICTIREPLACEGETB

        SWAP
        DROP
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)));

    test_case(
        "
        PUSHSLICE x4_
        NEWC
        STSLICE

        PUSHSLICE x5_
        NEWC
        STSLICE
        PUSHINT 99
        NEWDICT
        PUSHINT 8
        DICTISETB

        PUSHINT 99
        SWAP
        PUSHINT 8
        DICTIREPLACEGETB

        XCHG s2
        DROP
        ",
    )
    .expect_stack(
        Stack::new()
            .push(StackItem::boolean(true))
            .push(StackItem::Slice(SliceData::new(vec![0x50]))),
    );
}

#[test]
fn test_dict_addb_once() {
    test_case(
        "
        PUSHSLICE x5_
        NEWC
        STSLICE
        PUSHSLICE x7_
        NEWDICT
        PUSHINT 3
        DICTADDB

        SWAP
        DROP
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(true)));

    test_case(
        "
        PUSHSLICE x4_
        NEWC
        STSLICE

        PUSHSLICE x5_
        NEWC
        STSLICE
        PUSHSLICE x7_
        NEWDICT
        PUSHINT 3
        DICTSETB

        PUSHSLICE x7_
        SWAP
        PUSHINT 3
        DICTADDB

        SWAP
        DROP
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)));
}

#[test]
fn test_dict_addb_once_keywriter_to_uint() {
    test_case(
        "
        PUSHSLICE x5_
        NEWC
        STSLICE

        PUSHINT 99
        NEWDICT
        PUSHINT 8
        DICTUADDB

        SWAP
        DROP
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(true)));

    test_case(
        "
        PUSHSLICE x4_
        NEWC
        STSLICE

        PUSHSLICE x5_
        NEWC
        STSLICE
        PUSHINT 99
        NEWDICT
        PUSHINT 8
        DICTUSETB

        PUSHINT 99
        SWAP
        PUSHINT 8
        DICTUADDB

        SWAP
        DROP
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)));
}

#[test]
fn test_dict_addb_once_keywriter_to_int() {
    test_case(
        "
        PUSHSLICE x5_
        NEWC
        STSLICE

        PUSHINT 99
        NEWDICT
        PUSHINT 8
        DICTIADDB

        SWAP
        DROP
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(true)));

    test_case(
        "
        PUSHSLICE x4_
        NEWC
        STSLICE

        PUSHSLICE x5_
        NEWC
        STSLICE
        PUSHINT 99
        NEWDICT
        PUSHINT 8
        DICTISETB

        PUSHINT 99
        SWAP
        PUSHINT 8
        DICTIADDB

        SWAP
        DROP
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)));
}

#[test]
fn test_dict_addgetb_once() {
    test_case(
        "
        PUSHSLICE x5_
        NEWC
        STSLICE
        PUSHSLICE x7_
        NEWDICT
        PUSHINT 3
        DICTADDGETB

        SWAP
        DROP
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(true)));

    test_case(
        "
        PUSHSLICE x4_
        NEWC
        STSLICE

        PUSHSLICE x5_
        NEWC
        STSLICE
        PUSHSLICE x7_
        NEWDICT
        PUSHINT 3
        DICTSETB

        PUSHSLICE x7_
        SWAP
        PUSHINT 3
        DICTADDGETB

        XCHG s2
        DROP
        ",
    )
    .expect_stack(
        Stack::new()
            .push(StackItem::boolean(false))
            .push(StackItem::Slice(SliceData::new(vec![0x50]))),
    );
}

#[test]
fn test_dict_addgetb_once_keywriter_to_uint() {
    test_case(
        "
        PUSHSLICE x5_
        NEWC
        STSLICE
        PUSHINT 99
        NEWDICT
        PUSHINT 8
        DICTUADDGETB

        SWAP
        DROP
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(true)));

    test_case(
        "
        PUSHSLICE x4_
        NEWC
        STSLICE

        PUSHSLICE x5_
        NEWC
        STSLICE
        PUSHINT 99
        NEWDICT
        PUSHINT 8
        DICTUSETB

        PUSHINT 99
        SWAP
        PUSHINT 8
        DICTUADDGETB

        XCHG s2
        DROP
        ",
    )
    .expect_stack(
        Stack::new()
            .push(StackItem::boolean(false))
            .push(StackItem::Slice(SliceData::new(vec![0x50]))),
    );
}

#[test]
fn test_dict_addgetb_once_keywriter_to_int() {
    test_case(
        "
        PUSHSLICE x5_
        NEWC
        STSLICE
        PUSHINT 99
        NEWDICT
        PUSHINT 8
        DICTIADDGETB

        SWAP
        DROP
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(true)));

    test_case(
        "
        PUSHSLICE x4_
        NEWC
        STSLICE

        PUSHSLICE x5_
        NEWC
        STSLICE
        PUSHINT 99
        NEWDICT
        PUSHINT 8
        DICTISETB

        PUSHINT 99
        SWAP
        PUSHINT 8
        DICTIADDGETB

        XCHG s2
        DROP
        ",
    )
    .expect_stack(
        Stack::new()
            .push(StackItem::boolean(false))
            .push(StackItem::Slice(SliceData::new(vec![0x50]))),
    );
}

#[test]
fn test_dict_stores_into_builder_and_get() {
    test_case(
        "
        PUSHSLICE x4_
        PUSHINT 0
        NEWDICT
        PUSHINT 8
        DICTUSET
        PUSHSLICE x4_
        SWAP
        PUSHINT 1
        SWAP
        PUSHINT 8
        DICTUSET

        NEWC
        STDICT
        ENDC
        CTOS
        PLDDICT

        PUSHINT 0
        SWAP
        PUSHINT 8

        DICTUGET
        ",
    )
    .expect_stack(Stack::new().push(create::slice([0x40])).push(StackItem::boolean(true)));
}

#[test]
fn test_dict_pfxreplace_once() {
    test_case(
        "
        PUSHSLICE x5_
        PUSHSLICE x7_
        NEWDICT
        PUSHINT 3
        PFXDICTREPLACE

        SWAP
        DROP
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)));

    test_case(
        "
        PUSHSLICE x5_
        PUSHSLICE x7_
        NEWDICT
        PUSHINT 3
        PFXDICTSET
        THROWIFNOT 41

        PUSHSLICE x4_
        SWAP
        PUSHSLICE x7_
        SWAP
        PUSHINT 3
        PFXDICTREPLACE

        SWAP
        DROP
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(true)));
}

#[test]
fn test_dict_pfxadd_once() {
    test_case(
        "
        PUSHSLICE x5_
        PUSHSLICE x7_
        NEWDICT
        PUSHINT 3
        PFXDICTADD

        SWAP
        DROP
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(true)));

    test_case(
        "
        PUSHSLICE x4_

        PUSHSLICE x5_
        PUSHSLICE x7_
        NEWDICT
        PUSHINT 3
        DICTSET

        PUSHSLICE x7_
        SWAP
        PUSHINT 3
        PFXDICTADD

        SWAP
        DROP
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)));
}

#[test]
fn test_pfxdelete_on_empty_dict() {
    test_case(
        "
        PUSHSLICE x5_
        NEWDICT
        PUSHINT 3
        PFXDICTDEL
        SWAP
        DICTEMPTY
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)).push(StackItem::boolean(true)));
}

#[test]
fn test_dict_pfxfill_and_getq_once() {
    test_case(
        "
        PUSHSLICE x77
        PUSHSLICE x7_
        NEWDICT
        PUSHINT 3
        PFXDICTSET

        PUSHCONT {
            PUSHSLICE x7_
            SWAP
            PUSHINT 6
            PFXDICTGETQ
        }
        IF
        ",
    )
    .expect_stack(
        Stack::new()
            .push(StackItem::Slice(SliceData::new(vec![0x70])))
            .push(StackItem::boolean(false)),
    );

    test_case(
        "
        PUSHSLICE x5_
        PUSHSLICE x7_
        NEWDICT
        PUSHINT 3
        PFXDICTSET

        PUSHCONT {
            PUSHSLICE x77
            SWAP
            PUSHINT 3
            PFXDICTGETQ
        }
        IF
        ",
    )
    .expect_stack(
        Stack::new()
            .push(StackItem::Slice(SliceData::new(vec![0x70])))
            .push(StackItem::Slice(SliceData::new(vec![0x50])))
            .push(StackItem::Slice(SliceData::new(vec![0xBC])))
            .push(StackItem::boolean(true)),
    );
}

#[test]
fn test_dict_pfxfill_and_pfxset_prefix() {
    test_case(
        "
        PUSHSLICE x7C0      ; value
        PUSHSLICE x7_       ; key - 011
        NEWDICT             ; dictionary
        PUSHINT 3           ; bit_len
        PFXDICTSET          ; dict[011]=78_

        PUSHCONT {
            PUSHSLICE xC_   ; value
            SWAP
            PUSHSLICE x6_   ; key 01 - is shorter
            SWAP
            PUSHINT 3
            PFXDICTSET      ; value could not be set because key $01 is prefix of $011
            SWAP
            DROP
        }
        IF
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)));
}

#[test]
fn test_dict_pfxfill_and_pfxget_once() {
    test_case(
        "
        PUSHSLICE x7C0      ; value
        PUSHSLICE x7_       ; key - 011
        NEWDICT             ; dictionary
        PUSHINT 3           ; bit_len
        PFXDICTSET          ; dict[011]=78_

        PUSHCONT {
            PUSHSLICE x6_   ; key 01 - is shorter
            SWAP
            PUSHINT 3
            PFXDICTGET
        }
        IF
        ",
    )
    .expect_failure(ExceptionCode::CellUnderflow);

    test_case(
        "
        PUSHSLICE x5_       ; value
        PUSHSLICE x7_       ; key 011
        NEWDICT
        PUSHINT 3           ; bit_len
        PFXDICTSET

        PUSHCONT {
            PUSHSLICE x7C_  ; key is longer
            SWAP
            PUSHINT 3
            PFXDICTGET
        }
        IF
        ",
    )
    .expect_stack(
        Stack::new()
            .push(StackItem::Slice(SliceData::new(vec![0x70])))
            .push(StackItem::Slice(SliceData::new(vec![0x50])))
            .push(StackItem::Slice(SliceData::new(vec![0xE0]))),
    );
}

#[test]
fn test_dict_self_load() {
    test_case(
        "
        PUSHSLICE x4_
        PUSHINT 0 ; key 0
        NEWDICT   ; empty dictionary
        PUSHINT 8 ; bits
        DICTUSET  ; 00000000 <- x4_
        PUSHSLICE x4_
        SWAP
        PUSHINT 1 ; key 1
        SWAP
        PUSHINT 8 ; bits
        DICTUSET  ; 00000001 <- x4_

        NEWC
        STDICT
        ENDC
        CTOS
        LDDICT
        SWAP

        PUSHINT 0
        SWAP
        PUSHINT 8

        DICTUGET
        ",
    )
    .expect_stack(
        Stack::new()
            .push(dict_remainder(0xC0))
            .push(create::slice([0x40]))
            .push(StackItem::boolean(true)),
    );
}

#[test]
fn test_dict_load_simple() {
    test_case(
        "
        PUSHSLICE x5_

        PUSHSLICE x4_
        PUSHINT 0
        NEWDICT
        PUSHINT 8
        DICTUSET
        PUSHSLICE x4_
        SWAP
        PUSHINT 1
        SWAP
        PUSHINT 8
        DICTUSET

        NEWC
        STDICT
        STSLICE
        ENDC
        CTOS

        LDDICT
        SWAP

        PUSHINT 0
        SWAP
        PUSHINT 8

        DICTUGET
        ",
    )
    .expect_stack(
        Stack::new()
            .push(dict_remainder(0xA8))
            .push(create::slice([0x40]))
            .push(StackItem::boolean(true)),
    );
}

#[test]
fn test_dict_load_no_ref() {
    test_case(
        "
        PUSHSLICE xF_
        LDDICTS
    ",
    )
    .expect_failure(ExceptionCode::CellUnderflow);
}

#[test]
fn test_dict_load_no_dict() {
    test_case(
        "
        PUSHSLICE x4_
        LDDICT
    ",
    )
    .expect_stack(Stack::new().push(StackItem::None).push(StackItem::Slice(SliceData::default())));
}

#[test]
fn test_dict_skip_dict_load() {
    test_case(
        "
        PUSHSLICE x5_

        PUSHSLICE x4_
        PUSHINT 0
        NEWDICT
        PUSHINT 8
        DICTUSET
        PUSHSLICE x4_
        SWAP
        PUSHINT 1
        SWAP
        PUSHINT 8
        DICTUSET

        NEWC
        STDICT
        STSLICE
        ENDC
        CTOS

        SKIPDICT
        ",
    )
    .expect_stack(Stack::new().push(dict_remainder(0xA8)));
}

#[test]
fn test_dict_skip_dict_load_empty() {
    test_case(
        "
        PUSHSLICE x5_
        SKIPDICT
        ",
    )
    .expect_stack(Stack::new().push(dict_remainder(0x50)));
}

#[test]
fn test_dict_skip_dict_load_no_data() {
    test_case(
        "
        PUSHSLICE x8_
        SKIPDICT
        ",
    )
    .expect_failure(ExceptionCode::CellUnderflow);
}

#[test]
fn test_dict_load_and_drop_rest() {
    test_case(
        "
        PUSHSLICE x5_

        PUSHSLICE x4_   ; D[0]=x4_
        PUSHINT 0
        NEWDICT
        PUSHINT 8
        DICTUSET
        PUSHSLICE x4_   ; D[1]=x4_
        SWAP
        PUSHINT 1
        SWAP
        PUSHINT 8
        DICTUSET

        NEWC
        STDICT
        STSLICE
        ENDC
        CTOS

        PLDDICT

        PUSHINT 0
        SWAP
        PUSHINT 8

        DICTUGET
        ",
    )
    .expect_stack(Stack::new().push(create::slice([0x40])).push(StackItem::boolean(true)));
}

#[test]
fn test_dict_quiet_load_simple() {
    test_case(
        "
        PUSHSLICE x5_

        PUSHSLICE x4_
        PUSHINT 0
        NEWDICT
        PUSHINT 8
        DICTUSET
        PUSHSLICE x4_
        SWAP
        PUSHINT 1
        SWAP
        PUSHINT 8
        DICTUSET

        NEWC
        STDICT
        STSLICE
        ENDC
        CTOS

        LDDICTQ
        PUSHCONT {
            SWAP
            PUSHINT 0
            SWAP
            PUSHINT 8

            DICTUGET
        }
        IF
        ",
    )
    .expect_stack(
        Stack::new()
            .push(dict_remainder(0xA8))
            .push(create::slice([0x40]))
            .push(StackItem::boolean(true)),
    );
}

#[test]
fn test_dict_quiet_load_no_ref() {
    test_case(
        "
        PUSHSLICE xF_
        LDDICTQ
    ",
    )
    .expect_stack(Stack::new().push(create::slice([0xF0])).push(StackItem::boolean(false)));
}

#[test]
fn test_dict_quiet_load_and_drop_rest() {
    test_case(
        "
        PUSHSLICE x5_

        PUSHSLICE x4_
        PUSHINT 0
        NEWDICT
        PUSHINT 8
        DICTUSET
        PUSHSLICE x4_
        SWAP
        PUSHINT 1
        SWAP
        PUSHINT 8
        DICTUSET

        NEWC
        STDICT
        STSLICE
        ENDC
        CTOS

        PLDDICTQ
        PUSHCONT {
            PUSHINT 0
            SWAP
            PUSHINT 8

            DICTUGET
        }
        IF
        ",
    )
    .expect_stack(Stack::new().push(create::slice([0x40])).push(StackItem::boolean(true)));
}

#[test]
fn test_dict_quiet_preload_no_ref() {
    test_case(
        "
        PUSHSLICE xF_
        PLDDICTQ
        ",
    )
    .expect_stack(Stack::new().push(StackItem::boolean(false)));
}

// PUSHINT 10 -> 0x7A
// PUSHINT 12 -> 0x7C
const CREATE_PFXDICT_INSTRUCTIONS: &str = "
    PUSHSLICE x7A8_     ; value PUSHINT 10
    PUSHSLICE x5_       ; key 010
    NEWDICT
    PUSHINT 3
    PFXDICTSET
    THROWIFNOT 41
    PUSHSLICE x800C8_   ; value
    SWAP
    PUSHSLICE x7_       ; key 011
    SWAP
    PUSHINT 3
    PFXDICTSET
    THROWIFNOT 41
";

mod pfxdictgetjmp {
    use super::*;

    #[test]
    fn test_normal_flow() {
        test_case(format!(
            "
            {}
            PUSHSLICE x5_   ; jump by key
            SWAP
            PUSHINT 3
            PFXDICTGETJMP",
            CREATE_PFXDICT_INSTRUCTIONS
        ))
        .expect_stack(
            Stack::new()
                .push(StackItem::Slice(SliceData::new(vec![0x50])))
                .push(StackItem::Slice(SliceData::new_empty()))
                .push(StackItem::int(10)),
        );

        test_case(format!(
            "
            {}
            PUSHSLICE x7_   ; jump by key
            SWAP
            PUSHINT 3
            PFXDICTGETJMP",
            CREATE_PFXDICT_INSTRUCTIONS
        ))
        .expect_stack(
            Stack::new()
                .push(StackItem::Slice(SliceData::new(vec![0x70])))
                .push(StackItem::Slice(SliceData::new_empty()))
                .push(StackItem::int(12)),
        );
    }
}

mod pfxdictgetexec {
    use super::*;

    #[test]
    fn test_normal_flow() {
        test_case(format!(
            "
            PUSHSLICE x5_
            {}
            PUSHINT 3
            PFXDICTGETEXEC ",
            CREATE_PFXDICT_INSTRUCTIONS
        ))
        .expect_stack(
            Stack::new()
                .push(StackItem::Slice(SliceData::new(vec![0x50])))
                .push(StackItem::Slice(SliceData::new_empty()))
                .push(StackItem::int(10)),
        );

        test_case(format!(
            "
            PUSHSLICE x7_
            {}
            PUSHINT 3
            PFXDICTGETEXEC ",
            CREATE_PFXDICT_INSTRUCTIONS
        ))
        .expect_stack(
            Stack::new()
                .push(StackItem::Slice(SliceData::new(vec![0x70])))
                .push(StackItem::Slice(SliceData::new_empty()))
                .push(StackItem::int(12)),
        );
    }
}

mod dictpushconst {

    use super::*;
    use ton_block::{HashmapType, PfxHashmapE};

    #[test]
    fn test_normal_flow_present() {
        let mut dict = PfxHashmapE::with_bit_len(8);
        dict.set(SliceData::new(vec![0xFF]), &SliceData::new(vec![0x71, 0x80])).unwrap(); // PUSHINT 1
        dict.set(SliceData::new(vec![0xF4]), &SliceData::new(vec![0x72, 0x80])).unwrap(); // PUSHINT 2
        test_case_with_ref(
            "
            PUSHINT 5
            DICTPUSHCONST 8 ; bit_len
        ",
            dict.data().unwrap().clone(),
        )
        .expect_stack(
            Stack::new()
                .push(StackItem::int(5))
                .push(StackItem::Cell(dict.data().unwrap().clone()))
                .push(StackItem::int(8)),
        );
    }

    #[test]
    fn test_normal_flow_absent() {
        test_case(
            "
            PUSHINT 5
            DICTPUSHCONST 8 ; bit_len
        ",
        )
        .expect_failure(ExceptionCode::InvalidOpcode);
    }
}

mod pfxdictswitch {

    use super::*;
    use ton_block::{HashmapType, PfxHashmapE};

    #[test]
    fn test_normal_flow() {
        let mut dict = PfxHashmapE::with_bit_len(8);
        dict.set(SliceData::new(vec![0xFF]), &SliceData::new(vec![0x71, 0x80])).unwrap(); // PUSHINT 1
        dict.set(SliceData::new(vec![0xF4]), &SliceData::new(vec![0x72, 0x80])).unwrap(); // PUSHINT 2
        test_case_with_ref(
            "
            PUSHSLICE xFF_
            PFXDICTSWITCH 8 ; bit_len
        ",
            dict.data().unwrap().clone(),
        )
        .skip_fift_check()
        .expect_stack(
            Stack::new()
                .push(StackItem::Slice(SliceData::new(vec![0xFF])))
                .push(StackItem::Slice(SliceData::default()))
                .push(StackItem::int(1)),
        );

        test_case_with_ref(
            "
            PUSHSLICE x7_   ; it should stay untouch
            PUSHSLICE xF4_
            PFXDICTSWITCH 8 ; bit_len
        ",
            dict.data().unwrap().clone(),
        )
        .expect_stack(
            Stack::new()
                .push(StackItem::Slice(SliceData::new(vec![0x70])))
                .push(StackItem::Slice(SliceData::new(vec![0xF4])))
                .push(StackItem::Slice(SliceData::new_empty()))
                .push(StackItem::int(2)),
        );
    }
}

#[test]
fn test_dict_get_ref_with_no_ref() {
    test_case(
        "
        PUSHSLICE x_
        PUSHINT 0
        NEWDICT
        PUSHINT 8
        DICTUSET
        PUSHINT 0
        SWAP
        PUSHINT 8
        DICTUGETREF
    ",
    )
    .expect_failure(ExceptionCode::DictionaryError);

    test_case(
        "
        NEWC
        STSLICECONST xF_
        ENDC
        NEWC
        STSLICECONST xF_
        STREF
        ENDC

        PUSHINT 125
        NEWDICT
        PUSHINT 8
        DICTUSETREF

        PUSHINT 125
        SWAP
        PUSHINT 8
        DICTUGET
        NIP
    ",
    )
    .expect_item(StackItem::int(-1));

    test_case(
        "
        NEWC
        STSLICECONST xF_
        ENDC
        NEWC
        STSLICECONST xF_
        STREF
        ENDC
        CTOS

        PUSHINT 125
        NEWDICT
        PUSHINT 8
        DICTUSET

        PUSHINT 125
        SWAP
        PUSHINT 8
        DICTUGETREF
        NIP
    ",
    )
    .expect_failure(ExceptionCode::DictionaryError);

    test_case(
        "
        NEWC
        STSLICECONST xF_
        NEWC
        ENDCST
        ENDC
        CTOS

        PUSHINT 0
        NEWDICT
        PUSHINT 8
        DICTUSET

        PUSHINT 0
        SWAP
        PUSHINT 8
        DICTUGETREF
    ",
    )
    .expect_failure(ExceptionCode::DictionaryError);

    test_case(
        "
        PUSHSLICE x5

        PUSHSLICE x7
        NEWDICT
        PUSHINT 3
        DICTSET

        PUSHSLICE x4
        SWAP
        PUSHINT 3
        DICTGETREF
    ",
    )
    .expect_item(StackItem::int(0));

    test_case(
        "
        PUSHSLICE x5

        PUSHSLICE x7
        NEWDICT
        PUSHINT 3
        DICTSET

        PUSHSLICE x7
        SWAP
        PUSHINT 3
        DICTGETREF
    ",
    )
    .expect_failure(ExceptionCode::DictionaryError);

    test_case(
        "
        PUSHSLICE x5

        PUSHSLICE x7
        NEWDICT
        PUSHINT 3
        DICTSET

        PUSHSLICE x6
        PUSHSLICE x7
        XCHG s2
        PUSHINT 3
        DICTREPLACEREF
    ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);

    test_case(
        "
        PUSHSLICE x5_
        NEWC
        STSLICE
        ENDC

        PUSHINT 125
        NEWDICT
        PUSHINT 8
        DICTUSETREF

        PUSHINT 125
        SWAP
        PUSHINT 8
        DICTUGET
        NIP
    ",
    )
    .expect_item(StackItem::int(-1));

    test_case(
        "
        PUSHSLICE x7
        PUSHINT 125
        NEWDICT
        PUSHINT 8
        DICTUSET

        PUSHINT 125
        PUSHSLICE x5_
        NEWC
        STSLICE
        ENDC
        XCHG s2
        PUSHINT 8
        DICTUADDREF
        NIP
    ",
    )
    .expect_item(StackItem::int(0));

    test_case(
        "
        PUSHSLICE x7
        PUSHINT 125
        NEWDICT
        PUSHINT 8
        DICTUSET

        PUSHINT 125
        PUSHSLICE x5_
        NEWC
        STSLICE
        ENDC
        XCHG s2
        PUSHINT 8
        DICTUSETGETREF
    ",
    )
    .expect_failure(ExceptionCode::DictionaryError);

    test_case(
        "
        PUSHSLICE x5_
        NEWC
        STSLICE
        ENDC

        PUSHSLICE x7_
        PUSHINT 12345
        NEWC
        STU 32
        STSLICE
        ENDC
        PUSHINT 125
        NEWDICT
        PUSHINT 8
        DICTUSETREF

        PUSHINT 125
        SWAP
        PUSHINT 8
        DICTUADDREF
        NIP
    ",
    )
    .expect_item(StackItem::int(0));

    test_case(
        "
        PUSHSLICE x5_
        NEWC
        STSLICE
        ENDC

        PUSHSLICE x7_
        PUSHINT 12345
        NEWC
        STU 32
        STSLICE
        ENDC
        PUSHINT 125
        NEWDICT
        PUSHINT 8
        DICTUSETREF

        PUSHINT 125
        SWAP
        PUSHINT 8
        DICTUSETGETREF
        NIP
        NIP
    ",
    )
    .expect_item(StackItem::int(-1));

    test_case(
        "
        PUSHSLICE x7_
        PUSHINT 12345
        NEWC
        STU 32
        STSLICE
        ENDC
        PUSHINT 125
        NEWDICT
        PUSHINT 8
        DICTUSET
    ",
    )
    .expect_failure(ExceptionCode::TypeCheckError);

    test_case(
        "
        PUSHSLICE x7_
        PUSHINT 12345
        NEWC
        STU 32
        STSLICE
        ENDC

        PUSHSLICE x7
        PUSHINT 125
        NEWDICT
        PUSHINT 8
        DICTUSET

        PUSHINT 125
        SWAP
        PUSHINT 8
        DICTUADDREF
        NIP
    ",
    )
    .expect_item(StackItem::int(0));

    test_case(
        "
        PUSHSLICE x5_
        PUSHINT 12345
        NEWC
        STU 32
        STSLICE
        ENDC
        PUSHINT 125
        NEWDICT
        PUSHINT 8
        DICTUSETREF

        PUSHINT 125
        SWAP
        PUSHINT 8
        DICTUGETREF
        NIP
    ",
    )
    .expect_item(StackItem::int(-1));
}

mod subdict_with_prefix {
    use super::*;

    #[test]
    fn test_subdictget() {
        test_case(
            "
            PUSHSLICE x1280     ; DICTGET key

            PUSHSLICE x1280     ; prefix
            PUSHINT 4           ; subdict l

            PUSHSLICE x5_       ; value
            PUSHSLICE x1280     ; key
            NEWDICT
            PUSHINT 8
            DICTSET

            PUSHINT 8
            SUBDICTGET

            PUSHINT 8
            DICTGET
            DROP
            ",
        )
        .expect_item(StackItem::Slice(SliceData::new(vec![0x50])));

        test_case(
            "
            PUSHSLICE x18       ; prefix
            PUSHINT 4           ; subdict l

            PUSHSLICE x4_       ; value 3
            PUSHSLICE x0480     ; key

            PUSHSLICE x3_       ; value 2
            PUSHSLICE x1380     ; key

            PUSHSLICE x2_       ; value 1
            PUSHSLICE x1280     ; key
            NEWDICT
            PUSHINT 8
            DICTSET             ; 1

            PUSHINT 8
            DICTSET             ; 2

            PUSHINT 8
            DICTSET             ; 3

            PUSHINT 8
            SUBDICTGET

            DUP
            PUSHSLICE x0480     ; keys missing
            SWAP
            PUSHINT 8
            DICTGET
            DROP
            DUP
            PUSHSLICE x0680
            SWAP
            PUSHINT 8
            DICTGET
            DROP

            DUP
            PUSHSLICE x1380      ; keys exist
            SWAP
            PUSHINT 8
            DICTGET
            DROP
            SWAP
            PUSHSLICE x1280
            SWAP
            PUSHINT 8
            DICTGET
            DROP
            ",
        )
        .expect_stack(
            Stack::new()
                .push(StackItem::Slice(SliceData::new(vec![0x30])))
                .push(StackItem::Slice(SliceData::new(vec![0x20]))),
        );
    }

    #[test]
    fn test_subdictiget() {
        test_case(
            "
            PUSHINT 10          ; DICTIGET key

            ZERO                ; prefix
            PUSHINT 3           ; subdict l

            PUSHSLICE x5_       ; value
            PUSHINT 10          ; key
            NEWDICT
            PUSHINT 8
            DICTISET

            PUSHINT 8
            SUBDICTIGET

            PUSHINT 8
            DICTIGET
            DROP
            ",
        )
        .expect_item(StackItem::Slice(SliceData::new(vec![0x50])));
    }

    #[test]
    fn test_subdictuget() {
        test_case(
            "
            PUSHINT 10          ; DICTIGET key

            ZERO                ; prefix
            PUSHINT 3           ; subdict l

            PUSHSLICE x5_       ; value
            PUSHINT 10          ; key
            NEWDICT
            PUSHINT 8
            DICTUSET

            PUSHINT 8
            SUBDICTUGET

            PUSHINT 8
            DICTUGET
            DROP
            ",
        )
        .expect_item(StackItem::Slice(SliceData::new(vec![0x50])));
    }
}

mod subdict_without_prefix {
    use super::*;

    #[test]
    fn test_subdictrpget() {
        test_case(
            "
            PUSHSLICE x28       ; DICTGET key

            PUSHSLICE x1280     ; prefix
            PUSHINT 4           ; subdict l

            PUSHSLICE x5_       ; value
            PUSHSLICE x1280     ; key
            NEWDICT
            PUSHINT 8
            DICTSET

            PUSHINT 8
            SUBDICTRPGET

            PUSHINT 4
            DICTGET
            DROP
            ",
        )
        .expect_item(StackItem::Slice(SliceData::new(vec![0x50])));

        test_case(
            "
            PUSHSLICE x18       ; prefix
            PUSHINT 4           ; subdict l

            PUSHSLICE x4_       ; value 3
            PUSHSLICE x0480     ; key

            PUSHSLICE x3_       ; value 2
            PUSHSLICE x1380     ; key

            PUSHSLICE x2_       ; value 1
            PUSHSLICE x1280     ; key
            NEWDICT
            PUSHINT 8
            DICTSET             ; 1

            PUSHINT 8
            DICTSET             ; 2

            PUSHINT 8
            DICTSET             ; 3

            PUSHINT 8
            SUBDICTRPGET

            DUP
            PUSHSLICE x0480     ; keys missing
            SWAP
            PUSHINT 4
            DICTGET
            DROP
            DUP
            PUSHSLICE x48
            SWAP
            PUSHINT 4
            DICTGET
            DROP

            DUP
            PUSHSLICE x38      ; keys exist
            SWAP
            PUSHINT 4
            DICTGET
            DROP
            SWAP
            PUSHSLICE x28
            SWAP
            PUSHINT 4
            DICTGET
            DROP
            ",
        )
        .expect_stack(
            Stack::new()
                .push(StackItem::Slice(SliceData::new(vec![0x30])))
                .push(StackItem::Slice(SliceData::new(vec![0x20]))),
        );
    }

    #[test]
    fn test_subdictirpget() {
        test_case(
            "
            PUSHINT 10          ; DICTIGET key

            ZERO                ; prefix
            PUSHINT 3           ; subdict l

            PUSHSLICE x5_       ; value
            PUSHINT 10          ; key
            NEWDICT
            PUSHINT 8
            DICTISET

            PUSHINT 8
            SUBDICTIRPGET

            PUSHINT 5
            DICTIGET
            DROP
            ",
        )
        .expect_item(StackItem::Slice(SliceData::new(vec![0x50])));
    }

    #[test]
    fn test_subdicturpget() {
        test_case(
            "
            PUSHINT 10          ; DICTIGET key

            ZERO                ; prefix
            PUSHINT 3           ; subdict l

            PUSHSLICE x5_       ; value
            PUSHINT 10          ; key
            NEWDICT
            PUSHINT 8
            DICTUSET

            PUSHINT 8
            SUBDICTURPGET

            PUSHINT 5
            DICTUGET
            DROP
            ",
        )
        .expect_item(StackItem::Slice(SliceData::new(vec![0x50])));
    }
}
mod dict_optref {
    use super::*;

    #[test]
    fn test_get_with_slice() {
        test_case(
            "
            NEWC
            ENDC
            PUSHSLICE x00
            NEWDICT
            PUSHINT 8
            DICTSETGETOPTREF
            PUSHSLICE x00
            ROT
            PUSHINT 8
            DICTGETOPTREF
        ",
        )
        .expect_stack(Stack::new().push(StackItem::None).push(create::cell([0x80])));
    }

    #[test]
    fn test_get_with_int_normal() {
        test_case(
            "
            NEWC
            NEWC
            STBREFR
            ENDC
            CTOS
            PUSHINT 0
            NEWDICT
            PUSHINT 8
            DICTISET    ; set empty cell to dict[0:8]
            NULL
            PUSHINT 0
            ROT
            PUSHINT 8
            DICTISETGETOPTREF
            SWAP
            PUSHINT 0
            SWAP
            PUSHINT 8
            DICTIGETOPTREF
        ",
        )
        .expect_stack(Stack::new().push(create::cell([0x80])).push(StackItem::None));

        test_case(
            "
            NEWC
            NEWC
            STBREFR
            ENDC
            CTOS
            PUSHINT -10
            NEWDICT
            PUSHINT 8
            DICTISET    ; set empty cell to dict[0:8]
            NULL
            PUSHINT -10
            ROT
            PUSHINT 8
            DICTISETGETOPTREF
            SWAP
            PUSHINT -10
            SWAP
            PUSHINT 8
            DICTIGETOPTREF
        ",
        )
        .expect_stack(Stack::new().push(create::cell([0x80])).push(StackItem::None));
    }

    #[test]
    fn test_get_with_int_error() {
        test_case(
            "
            PUSHSLICE x_
            PUSHINT 0
            NEWDICT
            PUSHINT 8
            DICTISET    ; set empty cell to dict[0:8]
            PUSHINT 0
            SWAP
            PUSHINT 8
            DICTIGETOPTREF
        ",
        )
        .expect_failure(ExceptionCode::DictionaryError);
    }

    #[test]
    fn test_set_null_get_with_int_error() {
        test_case(
            "
            PUSHSLICE x_
            PUSHINT 0
            NEWDICT
            PUSHINT 8
            DICTISET    ; set empty cell to dict[0:8]
            NULL
            PUSHINT 0
            ROT
            PUSHINT 8
            DICTISETGETOPTREF
        ",
        )
        .expect_failure(ExceptionCode::DictionaryError);
    }

    #[test]
    fn test_get_with_uint() {
        test_case(
            "
            NEWC
            ENDC
            PUSHINT 0
            NEWDICT
            PUSHINT 8
            DICTUSETGETOPTREF
            PUSHINT 0
            ROT
            PUSHINT 8
            DICTUGETOPTREF
        ",
        )
        .expect_stack(Stack::new().push(StackItem::None).push(create::cell([0x80])));
    }
}

#[test]
fn test_stdict_with_null() {
    test_case(
        "
        NULL
        NEWC
        STDICT
        ENDC
    ",
    )
    .expect_item(create::cell([0x40]));
}

mod dict_add_replace_in_tree {
    use super::*;

    fn make_dict_test_case_int(
        slice: &str,
        key: &str,
        key_len: &str,
        prefix: &str,
        throw: &str,
    ) -> String {
        format!(
            "
            PUSHSLICE {}
            SWAP
            PUSHINT {}
            SWAP
            PUSHINT {}
            DICT{}REPLACE
            THROWIF 41{}

            PUSHSLICE {}
            SWAP
            PUSHINT {}
            SWAP
            PUSHINT {}
            DICT{}REPLACEGET
            THROWIF 42{}

            PUSHSLICE {}
            SWAP
            PUSHINT {}
            SWAP
            PUSHINT {}
            DICT{}ADD
            THROWIFNOT 43{}

            PUSHSLICE {}
            SWAP
            PUSHINT {}
            SWAP
            PUSHINT {}
            DICT{}ADD
            THROWIF 44{}

            PUSHSLICE {}
            SWAP
            PUSHINT {}
            SWAP
            PUSHINT {}
            DICT{}ADDGET
            THROWIF 45{}
            DROP

            PUSHSLICE {}
            SWAP
            PUSHINT {}
            SWAP
            PUSHINT {}
            DICT{}REPLACE
            THROWIFNOT 46{}

            PUSHSLICE {}
            SWAP
            PUSHINT {}
            SWAP
            PUSHINT {}
            DICT{}REPLACEGET
            THROWIFNOT 47{}
            DROP
            ",
            slice,
            key,
            key_len,
            prefix,
            throw,
            slice,
            key,
            key_len,
            prefix,
            throw,
            slice,
            key,
            key_len,
            prefix,
            throw,
            slice,
            key,
            key_len,
            prefix,
            throw,
            slice,
            key,
            key_len,
            prefix,
            throw,
            slice,
            key,
            key_len,
            prefix,
            throw,
            slice,
            key,
            key_len,
            prefix,
            throw
        )
    }

    #[test]
    fn test_dict_add_with_replace_in_tree() {
        let table = [
            ("x12345_", "255"),
            ("x23456_", "252"),
            ("x34567_", "243"),
            ("x45678_", "240"),
            ("x56789_", "207"),
            ("x67891_", "204"),
            ("x78912_", "195"),
            ("x89123_", "192"),
            ("x11111_", "0"),
            ("x22222_", "15"),
        ];
        let mut buffer_uint = String::from("NEWDICT\n");
        let mut buffer_int = String::from("NEWDICT\n");

        let mut index = 0;
        while index != table.len() {
            let (slice, key) = table[index];
            buffer_uint +=
                make_dict_test_case_int(slice, key, "8", "U", index.to_string().as_str()).as_str();
            buffer_int +=
                make_dict_test_case_int(slice, key, "16", "I", index.to_string().as_str()).as_str();
            index += 1;
        }
        buffer_uint.push_str("DROP");
        buffer_int.push_str("DROP");

        test_case(buffer_uint.as_str()).expect_empty_stack();
        test_case(buffer_int.as_str()).expect_empty_stack();
    }
}

mod dict_get_iter_in_tree {
    use super::*;

    fn add_to_dict(slice: &str, key: &str, key_len: &str, prefix: &str, throw: &str) -> String {
        format!(
            "
            PUSHSLICE {}
            SWAP
            PUSHINT {}
            SWAP
            PUSHINT {}
            DICT{}ADD
            THROWIFNOT 41{}
        ",
            slice, key, key_len, prefix, throw
        )
    }

    fn get_next_from_dict(
        key: &str,
        key_len: &str,
        suffix: &str,
        throw: &str,
        expect_key: &str,
    ) -> String {
        format!(
            "
            DUP
            PUSHINT {}
            SWAP
            PUSHINT {}
            DICT{}
            THROWIFNOT 42{}
            PUSHINT {}
            EQUAL
            THROWIFNOT 43{}
            DROP
        ",
            key, key_len, suffix, throw, expect_key, throw
        )
    }

    #[test]
    fn test_dict_getnext_in_tree() {
        let table_uint = [
            ("x12345_", "255", "254"),
            ("x23456_", "252", "251"),
            ("x34567_", "243", "242"),
            ("x45678_", "240", "239"),
            ("x56789_", "207", "206"),
            ("x67891_", "204", "203"),
            ("x78912_", "195", "194"),
            ("x89123_", "192", "160"),
            ("x11111_", "1", "0"),
            ("x22222_", "16", "1"),
        ];
        let table_int = [
            ("x12345_", "-120", "-121"),
            ("x23456_", "50", "49"),
            ("x34567_", "-122", "-123"),
            ("x45678_", "-64", "-120"),
            ("x56789_", "-10", "-11"),
            ("x67891_", "-31", "-32"),
            ("x78912_", "-5", "-9"),
            ("x89123_", "13", "0"),
            ("x11111_", "0", "-1"),
            ("x22222_", "-1", "-2"),
        ];
        let mut buffer_uint = String::from("NEWDICT\n");
        let mut buffer_int = String::from("NEWDICT\n");

        let mut index = 0;
        while index != table_uint.len() && index != table_int.len() {
            let (slice, key, _) = table_uint[index];
            buffer_uint += add_to_dict(slice, key, "8", "U", index.to_string().as_str()).as_str();
            let (slice, key, _) = table_int[index];
            buffer_int += add_to_dict(slice, key, "8", "I", index.to_string().as_str()).as_str();
            index += 1;
        }

        let mut index = 0;
        while index != table_uint.len() && index != table_int.len() {
            let (_, key, prev) = table_uint[index];
            buffer_uint +=
                get_next_from_dict(prev, "8", "UGETNEXT", index.to_string().as_str(), key).as_str();
            let (_, key, prev) = table_int[index];
            buffer_int +=
                get_next_from_dict(prev, "8", "IGETNEXT", index.to_string().as_str(), key).as_str();
            index += 1;
        }

        buffer_uint += "DUP
                        PUSHINT 17
                        SWAP
                        PUSHINT 8
                        DICTUGETNEXT
                        THROWIFNOT 440
                        PUSHINT 192
                        EQUAL
                        THROWIFNOT 450
                        DROP
                        ";
        buffer_uint += "DUP
                        PUSHINT 255
                        SWAP
                        PUSHINT 8
                        DICTUGETNEXT
                        THROWIF 460
                        ";

        buffer_int += "DUP
                       PUSHINT -128
                       SWAP
                       PUSHINT 8
                       DICTIGETNEXT
                       THROWIFNOT 440
                       PUSHINT -122
                       EQUAL
                       THROWIFNOT 450
                       DROP
                       ";
        buffer_int += "DUP
                       PUSHINT 50
                       SWAP
                       PUSHINT 8
                       DICTIGETNEXT
                       THROWIF 460
                       ";

        buffer_uint.push_str("DROP");
        buffer_int.push_str("DROP");
        test_case(buffer_uint.as_str()).expect_empty_stack();
        test_case(buffer_int.as_str()).expect_empty_stack();
    }

    #[test]
    fn test_dict_getnexteq_in_tree() {
        let table_uint = [
            ("x12345_", "254", "253"),
            ("x23456_", "252", "251"),
            ("x34567_", "243", "242"),
            ("x45678_", "240", "240"),
            ("x56789_", "207", "207"),
            ("x67891_", "204", "203"),
            ("x78912_", "195", "194"),
            ("x89123_", "192", "160"),
            ("x11111_", "1", "0"),
            ("x22222_", "16", "2"),
        ];
        let table_int = [
            ("x12345_", "-120", "-121"),
            ("x23456_", "50", "49"),
            ("x34567_", "15", "15"),
            ("x45678_", "-64", "-119"),
            ("x56789_", "-10", "-10"),
            ("x67891_", "-31", "-31"),
            ("x78912_", "-5", "-9"),
            ("x89123_", "13", "1"),
            ("x11111_", "0", "-1"),
            ("x22222_", "-2", "-3"),
        ];
        let mut buffer_uint = String::from("NEWDICT\n");
        let mut buffer_int = String::from("NEWDICT\n");

        let mut index = 0;
        while index != table_uint.len() && index != table_int.len() {
            let (slice, key, _) = table_uint[index];
            buffer_uint += add_to_dict(slice, key, "8", "U", index.to_string().as_str()).as_str();
            let (slice, key, _) = table_int[index];
            buffer_int += add_to_dict(slice, key, "8", "I", index.to_string().as_str()).as_str();
            index += 1;
        }

        let mut index = 0;
        while index != table_uint.len() && index != table_int.len() {
            let (_, key, prev) = table_uint[index];
            buffer_uint +=
                get_next_from_dict(prev, "8", "UGETNEXTEQ", index.to_string().as_str(), key)
                    .as_str();
            let (_, key, prev) = table_int[index];
            buffer_int +=
                get_next_from_dict(prev, "8", "IGETNEXTEQ", index.to_string().as_str(), key)
                    .as_str();
            index += 1;
        }

        buffer_uint += "DUP
                        PUSHINT 255
                        SWAP
                        PUSHINT 8
                        DICTUGETNEXTEQ
                        THROWIF 460
                        ";

        buffer_int += "DUP
                       PUSHINT 51
                       SWAP
                       PUSHINT 8
                       DICTIGETNEXTEQ
                       THROWIF 460
                       ";

        buffer_uint.push_str("DROP");
        buffer_int.push_str("DROP");
        test_case(buffer_uint.as_str()).expect_empty_stack();
        test_case(buffer_int.as_str()).expect_empty_stack();
    }

    #[test]
    fn test_dict_getprev_in_tree() {
        let table_uint = [
            ("x12345_", "254", "255"),
            ("x23456_", "252", "253"),
            ("x34567_", "243", "244"),
            ("x45678_", "240", "241"),
            ("x56789_", "207", "208"),
            ("x67891_", "204", "205"),
            ("x78912_", "195", "196"),
            ("x89123_", "192", "193"),
            ("x11111_", "10", "14"),
            ("x22222_", "16", "192"),
        ];
        let table_int = [
            ("x12345_", "-120", "-119"),
            ("x23456_", "50", "127"),
            ("x34567_", "-122", "-121"),
            ("x45678_", "-64", "-63"),
            ("x56789_", "-10", "-9"),
            ("x67891_", "-31", "-27"),
            ("x78912_", "-5", "-2"),
            ("x89123_", "13", "14"),
            ("x11111_", "0", "1"),
            ("x22222_", "-1", "0"),
        ];
        let mut buffer_uint = String::from("NEWDICT\n");
        let mut buffer_int = String::from("NEWDICT\n");

        let mut index = 0;
        while index != table_uint.len() && index != table_int.len() {
            let (slice, key, _) = table_uint[index];
            buffer_uint += add_to_dict(slice, key, "8", "U", index.to_string().as_str()).as_str();
            let (slice, key, _) = table_int[index];
            buffer_int += add_to_dict(slice, key, "8", "I", index.to_string().as_str()).as_str();
            index += 1;
        }

        let mut index = 0;
        while index != table_uint.len() && index != table_int.len() {
            let (_, key, prev) = table_uint[index];
            buffer_uint +=
                get_next_from_dict(prev, "8", "UGETPREV", index.to_string().as_str(), key).as_str();
            let (_, key, prev) = table_int[index];
            buffer_int +=
                get_next_from_dict(prev, "8", "IGETPREV", index.to_string().as_str(), key).as_str();
            index += 1;
        }

        buffer_uint += "DUP
                        PUSHINT 0
                        SWAP
                        PUSHINT 8
                        DICTUGETPREV
                        THROWIF 460
                        ";
        buffer_uint += "DUP
                        PUSHINT 10
                        SWAP
                        PUSHINT 8
                        DICTUGETPREV
                        THROWIF 470
                        ";

        buffer_int += "DUP
                       PUSHINT -128
                       SWAP
                       PUSHINT 8
                       DICTIGETPREV
                       DUMPSTK
                       THROWIF 480
                       ";
        buffer_int += "DUP
                       PUSHINT -122
                       SWAP
                       PUSHINT 8
                       DICTIGETPREV
                       THROWIF 490
                       ";

        buffer_uint.push_str("DROP");
        buffer_int.push_str("DROP");
        test_case(buffer_uint.as_str()).expect_empty_stack();
        test_case(buffer_int.as_str()).expect_empty_stack();
    }

    #[test]
    fn test_dict_getpreveq_in_tree() {
        let table_uint = [
            ("x12345_", "190", "191"),
            ("x23456_", "252", "253"),
            ("x34567_", "243", "244"),
            ("x45678_", "240", "240"),
            ("x56789_", "207", "207"),
            ("x67891_", "254", "255"),
            ("x78912_", "195", "196"),
            ("x89123_", "192", "193"),
            ("x11111_", "1", "14"),
            ("x22222_", "16", "189"),
        ];
        let table_int = [
            ("x12345_", "-120", "-119"),
            ("x23456_", "50", "51"),
            ("x34567_", "-1", "0"),
            ("x45678_", "-64", "-64"),
            ("x56789_", "-10", "-9"),
            ("x67891_", "-31", "-27"),
            ("x78912_", "-5", "-2"),
            ("x89123_", "13", "14"),
            ("x11111_", "1", "2"),
            ("x22222_", "-127", "-126"),
        ];
        let mut buffer_uint = String::from("NEWDICT\n");
        let mut buffer_int = String::from("NEWDICT\n");

        let mut index = 0;
        while index != table_uint.len() && index != table_int.len() {
            let (slice, key, _) = table_uint[index];
            buffer_uint += add_to_dict(slice, key, "8", "U", index.to_string().as_str()).as_str();
            let (slice, key, _) = table_int[index];
            buffer_int += add_to_dict(slice, key, "8", "I", index.to_string().as_str()).as_str();
            index += 1;
        }

        let mut index = 0;
        while index != table_uint.len() && index != table_int.len() {
            let (_, key, prev) = table_uint[index];
            buffer_uint +=
                get_next_from_dict(prev, "8", "UGETPREVEQ", index.to_string().as_str(), key)
                    .as_str();
            let (_, key, prev) = table_int[index];
            buffer_int +=
                get_next_from_dict(prev, "8", "IGETPREVEQ", index.to_string().as_str(), key)
                    .as_str();
            index += 1;
        }

        buffer_uint += "DUP
                        PUSHINT 0
                        SWAP
                        PUSHINT 8
                        DICTUGETPREVEQ
                        THROWIF 460
                        ";

        buffer_int += "DUP
                       PUSHINT -128
                       SWAP
                       PUSHINT 8
                       DICTIGETPREVEQ
                       THROWIF 490
                       ";

        buffer_uint.push_str("DROP");
        buffer_int.push_str("DROP");
        test_case(buffer_uint.as_str()).expect_empty_stack();
        test_case(buffer_int.as_str()).expect_empty_stack();
    }
}

#[test]
fn test_pruned_access() {
    let make_pruned_cell = "
        NEWC
        STSLICECONST x0101
        PUSHINT 256
        STZEROES
        STSLICECONST x0000
        TRUE
        ENDXC
    ";
    let dbl_int = [
        "DICTUGET",
        "DICTUGETREF",
        "DICTUDEL",
        "DICTUDELGET",
        "DICTUDELGETREF",
        "DICTUGETJMP",
        "DICTUGETJMPZ",
        "DICTUGETEXEC",
        "DICTUGETEXECZ",
    ];
    let set_slice = ["DICTUADD", "DICTUADDGET", "DICTUSET", "DICTUREPLACE"];
    let set_cell = ["DICTUADDREF", "DICTUADDGETREF", "DICTUSETREF", "DICTUREPLACEREF"];
    let set_builder = ["DICTUADDB", "DICTUADDGETB", "DICTUSETB", "DICTUREPLACEB"];
    let pfx_get = ["PFXDICTGETQ", "PFXDICTGET", "PFXDICTGETJMP", "PFXDICTGETEXEC"];
    let pfx_set = ["PFXDICTSET", "PFXDICTREPLACE", "PFXDICTADD"];
    let pfx_del = ["PFXDICTDEL"];
    let dict_access: [(&str, &[&str]); 4] = [
        ("ZERO SWAP TWO", &dbl_int),
        ("PUSHSLICE xF ZERO ROT TWO", &set_slice),
        ("NEWC ENDC ZERO ROT TWO", &set_cell),
        ("NEWC ZERO ROT TWO", &set_builder),
    ];
    let pfx_access: [(&str, &[&str]); 3] = [
        ("PUSHSLICE x6_ SWAP PUSHINT 3", &pfx_get),
        ("PUSHSLICE x77 PUSHSLICE x7_ ROT PUSHINT 3", &pfx_set),
        ("PUSHSLICE x5_ SWAP PUSHINT 3", &pfx_del),
    ];
    for (prep, ops) in dict_access {
        for op in ops {
            let code = make_pruned_cell.to_string() + prep + " " + op;
            test_case(code).expect_failure(ExceptionCode::CellUnderflow);
        }
    }
    for (prep, ops) in pfx_access {
        for op in ops {
            let code = make_pruned_cell.to_string() + prep + " " + op;
            test_case(code).expect_failure(ExceptionCode::CellUnderflow);
        }
    }
}
