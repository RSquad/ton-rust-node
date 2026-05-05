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
use ton_assembler::compile_code_to_cell;
use ton_block::{
    ed25519_generate_private_key, AnycastInfo, BuilderData, Cell, CurrencyCollection,
    ExceptionCode, HashmapE, HashmapType, IBitstring, InternalMessageHeader, Message, MsgAddress,
    MsgAddressInt, Result, Serializable, Sha256, SliceData, StateInit, StorageUsageCalc,
    ACTION_CHANGE_LIB, ACTION_RESERVE, ACTION_SEND_MSG, ACTION_SET_CODE, ED25519_PUBLIC_KEY_LENGTH,
    ED25519_SIGNATURE_LENGTH,
};
use ton_vm::{
    boolean,
    executor::{serialize_currency_collection, BehaviorModifiers},
    int,
    stack::{integer::IntegerData, Stack, StackItem},
};

mod common;
use common::*;
use rand::RngCore;

fn gen_test_tree_of_cells() -> Cell {
    let mut random = rand::thread_rng();
    let mut buffer = [0u8; 127];
    //test cell with data and one not empty reference
    let mut builder = BuilderData::new();
    random.fill_bytes(&mut buffer[..]);
    builder.append_raw(&buffer, buffer.len() * 8).unwrap();
    let mut ref0 = BuilderData::new();
    random.fill_bytes(&mut buffer[..]);
    ref0.append_raw(&buffer, buffer.len() * 8).unwrap();
    builder.checked_append_reference(ref0.into_cell().unwrap()).unwrap();
    builder.into_cell().unwrap()
}

#[test]
fn test_chksignu_real() {
    let pair = ed25519_generate_private_key().unwrap();

    //test cell with data and one not empty reference
    let test_cell = gen_test_tree_of_cells();
    let cell_hash = test_cell.repr_hash();

    //sign hash of data cell
    let signature = pair.sign(cell_hash.as_slice()).to_vec();

    //put signature to separate slice
    let len = signature.len() * 8;
    let signature = SliceData::from_raw(signature, len);

    //put public key to integer
    let pub_key =
        BuilderData::with_raw(pair.verifying_key().to_vec(), ED25519_PUBLIC_KEY_LENGTH * 8)
            .unwrap();

    //put hash to integer
    let hash = BuilderData::with_raw(cell_hash.as_slice().to_vec(), 256).unwrap();

    test_case_with_refs(
        "
        PUSHREFSLICE
        PLDU 256
        PUSHREFSLICE
        PUSHREFSLICE
        PLDU 256
        NOP
        ;s0 - pub key: integer
        ;s1 - signature: slice
        ;s2 - hash: integer
        CHKSIGNU
    ",
        vec![
            hash.into_cell().unwrap(),
            signature.into_cell().unwrap(),
            pub_key.into_cell().unwrap(),
        ],
    )
    .expect_stack(Stack::new().push(int!(-1)));

    test_case("
        PUSHINT 15
        PUSHCONT {
            PUSHINT 66217541034200756890641849847588029095699779625619746207976976137706939289808
            PUSHSLICE xfb53f9005a9e7c91c7dc8fcaeecb2dd0d5af17703042cf4daf0c7ec7bc1da281e4f0b3c748bace798548e65697f52968848d830f6015c0709d8fad51d421c304
            PUSHINT 15336109783281190428388939426462642574584905613548735486866417552072882909493
            CHKSIGNU
        }  
        REPEAT
    ")
    .expect_success()
    .expect_gas(1000000000, 1000000000, 0, 999977750)
    .expect_int_stack(&[-1; 15]);
}

#[test]
fn test_chksignu_always() {
    let pair = ed25519_generate_private_key().unwrap();

    //test cell with data and one not empty reference
    let test_cell = gen_test_tree_of_cells();
    let cell_hash = test_cell.repr_hash();

    //fake signature of data cell
    let signature = SliceData::from_raw(vec![0; 64], 512).into_cell().unwrap();

    //put public key to integer
    let pub_key = SliceData::from_raw(pair.verifying_key().to_vec(), ED25519_PUBLIC_KEY_LENGTH * 8)
        .into_cell()
        .unwrap();

    //put hash to integer
    let hash = SliceData::from_raw(cell_hash.as_slice().to_vec(), 256).into_cell().unwrap();

    let modifiers = BehaviorModifiers { chksig_always_succeed: true };

    let code = "
        PUSHREFSLICE
        PLDU 256
        PUSHREFSLICE
        PUSHREFSLICE
        PLDU 256
        NOP
        ;s0 - pub key: integer
        ;s1 - signature: slice
        ;s2 - hash: integer
        CHKSIGNU
    ";

    test_case_with_refs(code, vec![hash.clone(), signature.clone(), pub_key.clone()])
        .expect_stack(Stack::new().push(int!(0)));

    test_case_with_refs(code, vec![hash, signature, pub_key])
        .with_behavior_modifiers(modifiers.clone())
        .expect_stack(Stack::new().push(int!(-1)));

    test_case("
        PUSHINT 66217541034200756890641849847588029095699779625619746207976976137706939289808
        PUSHSLICE xfb53f9005a9e7c91c7dc8fcaeecb2dd0d5af17703042cf4daf0c7ec7bc1da281e4f0b3c748bace798548e65697f52968848d830f6015c0709d8fad51d421c304
        PUSHINT 0
        CHKSIGNU
    ")
    .expect_stack(Stack::new().push(int!(0)));

    test_case("
        PUSHINT 66217541034200756890641849847588029095699779625619746207976976137706939289808
        PUSHSLICE xfb53f9005a9e7c91c7dc8fcaeecb2dd0d5af17703042cf4daf0c7ec7bc1da281e4f0b3c748bace798548e65697f52968848d830f6015c0709d8fad51d421c304
        PUSHINT 0
        CHKSIGNU
    ")
    .with_behavior_modifiers(modifiers)
    .expect_stack(Stack::new().push(int!(-1)));
}

#[test]
fn test_chksigns_underflow() {
    test_case(
        "
        PUSHSLICE x00
        PUSHSLICE x00
        PUSHINT 0
        CHKSIGNS
    ",
    )
    .expect_failure(ExceptionCode::CellUnderflow);

    test_case("
        PUSHSLICE x01_
        PUSHSLICE x00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000
        PUSHINT 0
        CHKSIGNS
    ")
    .expect_failure(ExceptionCode::CellUnderflow);
}

fn generate_tree_and_hash() -> (Cell, IntegerData) {
    let test_cell = gen_test_tree_of_cells();

    let hash_int = IntegerData::from_u256(test_cell.repr_hash()).unwrap();

    (test_cell, hash_int)
}

fn generate_tree_and_sha256() -> (Cell, IntegerData) {
    let test_slice = gen_test_tree_of_cells();
    let mut hasher = Sha256::new();
    hasher.update(test_slice.data());
    let sha256_hash = IntegerData::from_u256(hasher.finalize()).unwrap();

    (test_slice, sha256_hash)
}

fn call_hash_primitive(code: &str) {
    for _ in 0..10 {
        let (cell, hash) = generate_tree_and_hash();
        test_case_with_ref(code, cell).expect_item(StackItem::int(hash));
    }
}

fn call_sha256u(code: &str) {
    for _ in 0..10 {
        let (slice, hash) = generate_tree_and_sha256();
        test_case_with_ref(code, slice).expect_item(StackItem::int(hash));
    }
}

#[test]
fn test_hashbu() {
    call_hash_primitive(
        "
        PUSHREFSLICE
        NEWC
        STSLICE
        HASHBU
    ",
    );
    expect_exception("HASHBU", ExceptionCode::StackUnderflow);
    expect_exception("NEWC ENDC HASHBU", ExceptionCode::TypeCheckError);
    expect_exception("ZERO HASHBU", ExceptionCode::TypeCheckError);
}

#[test]
fn test_hashcu() {
    call_hash_primitive(
        "
        PUSHREF
        HASHCU
    ",
    );
    expect_exception("HASHCU", ExceptionCode::StackUnderflow);
    expect_exception("NEWC HASHCU", ExceptionCode::TypeCheckError);
    expect_exception("ZERO HASHCU", ExceptionCode::TypeCheckError);
}

#[test]
fn test_hashsu() {
    call_hash_primitive(
        "
        PUSHREFSLICE
        HASHSU
    ",
    );
    expect_exception("HASHSU", ExceptionCode::StackUnderflow);
    expect_exception("NEWC HASHSU", ExceptionCode::TypeCheckError);
    expect_exception("ZERO HASHSU", ExceptionCode::TypeCheckError);
}

#[test]
fn test_sha256u() {
    call_sha256u(
        "
        PUSHREFSLICE
        SHA256U
    ",
    );
}

#[test]
fn test_sha256u_cell_underflow() {
    expect_exception("PUSHINT 5 NEWC STI 7 ENDC CTOS SHA256U", ExceptionCode::CellUnderflow);
    expect_exception("PUSHSLICE xFF_ SHA256U", ExceptionCode::CellUnderflow);
    expect_exception("PUSHSLICE x05_ SHA256U", ExceptionCode::CellUnderflow);
}

#[test]
fn test_sha256u_stack_underflow() {
    expect_exception("SHA256U", ExceptionCode::StackUnderflow);
    expect_exception("STSLICECONST x05_ SHA256U", ExceptionCode::StackUnderflow);
}

#[test]
fn test_sha256u_type_error() {
    expect_exception("PUSHINT 5 SHA256U", ExceptionCode::TypeCheckError)
}

fn call_chksignu(hash_primitive: &str, push_primitive: &str) {
    let (slice, hash) = generate_tree_and_hash();
    let pair = ed25519_generate_private_key().unwrap();

    let cell = hash.as_slice(256, false, true).unwrap();

    let hash_bytes = cell.get_bytestring(0);
    //sign hash of tree of cells
    let signature = pair.sign(&hash_bytes).to_vec();
    let signature = BuilderData::with_raw(signature, ED25519_SIGNATURE_LENGTH * 8).unwrap();
    let key_slice =
        BuilderData::with_raw(pair.verifying_key().to_vec(), ED25519_PUBLIC_KEY_LENGTH * 8)
            .unwrap()
            .into_cell()
            .unwrap();

    test_case_with_refs(
        &format!(
            "
        PUSHREFSLICE
        PUSHREFSLICE
        {push_primitive}
        NOP
        {hash_primitive}
        XCHG s2
        LDU 256
        ENDS
        CHKSIGNU
    ",
            push_primitive = push_primitive,
            hash_primitive = hash_primitive
        ),
        vec![key_slice, signature.into_cell().unwrap(), slice],
    )
    .expect_item(int!(-1));
}

#[test]
fn test_chksignu_error() {
    expect_exception("CHKSIGNU", ExceptionCode::StackUnderflow);
    expect_exception("NULL CHKSIGNU", ExceptionCode::StackUnderflow);
    expect_exception("NULL NULL CHKSIGNU", ExceptionCode::StackUnderflow);

    expect_exception(
        "
        PUSHINT 123456
        PUSHSLICE x123
        PUSHINT 987654
        CHKSIGNU
    ",
        ExceptionCode::CellUnderflow,
    );

    expect_exception(
        "
        NULL
        PUSHSLICE x123
        PUSHINT 987654
        CHKSIGNU
    ",
        ExceptionCode::TypeCheckError,
    );

    expect_exception(
        "
        PUSHINT 123456
        NULL
        PUSHINT 987654
        CHKSIGNU
    ",
        ExceptionCode::TypeCheckError,
    );

    expect_exception(
        "
        PUSHINT 123456
        PUSHSLICE x123
        NULL
        CHKSIGNU
    ",
        ExceptionCode::TypeCheckError,
    );
}

#[test]
fn test_chksignu_bad_slice() {
    let signature = SliceData::new(vec![0xAA; 65]).into_cell().unwrap();
    let code = "
        PUSHINT 1234567
        PUSHREFSLICE
        PUSHINT 987654
        CHKSIGNU
    ";

    test_case_with_ref(code, signature).expect_item(boolean!(false));
}

#[test]
fn test_chksignu_bad_pubkey() {
    let code = "
        PUSHINT 1234567
        PUSHSLICE x0000000000000000000000000000000000000000000000000000000000bc614e00000000000000000000000000000000000000000000000000000000075bcd15
        PUSHINT 123456
        CHKSIGNU
    ";

    test_case(code).expect_item(boolean!(false));
}

#[test]
fn test_hashbu_and_chksign() {
    call_chksignu("NEWC STSLICE HASHBU", "PUSHREFSLICE");
}

#[test]
fn test_hashcu_and_chksign() {
    call_chksignu("HASHCU", "PUSHREF");
}

#[test]
fn test_hashsu_and_chksign() {
    call_chksignu("HASHSU", "PUSHREFSLICE");
}

#[test]
fn test_sendrawmsg_stackunderflow() {
    expect_exception("PUSHINT 0 SENDRAWMSG", ExceptionCode::StackUnderflow);
    expect_exception("SENDRAWMSG", ExceptionCode::StackUnderflow);
    expect_exception("NEWC ENDC SENDRAWMSG", ExceptionCode::StackUnderflow);
}

#[test]
fn test_sendrawmsg_too_big_int() {
    expect_exception(
        "NEWC
        ENDC
        PUSHINT 256
        SENDRAWMSG",
        ExceptionCode::RangeCheckError,
    );
}

#[test]
fn test_sendrawmsg_wrong_argument_order() {
    expect_exception(
        "PUSHINT 0
        NEWC
        ENDC
        SENDRAWMSG",
        ExceptionCode::TypeCheckError,
    );
}

#[test]
fn test_two_sendrawmsg_with_parsing() {
    test_case(format!(
        "
        ; init c5 register with empty cell (by spec)
        NEWC
        ENDC
        POPCTR c5

        ; create fake msg cell
        PUSHINT 12345
        NEWC
        STU 32
        ENDC
        PUSHINT 99
        SENDRAWMSG

        ; create another fake msg cell
        PUSHINT 67890
        NEWC
        STU 32
        ENDC
        PUSHINT 255
        SENDRAWMSG

        ; check:
        ; c5 =  cell (tag(u32) + mode(u8))
        ;       cell.ref0 - cell with prev action
        ;       cell.ref1 - cell with msg
        PUSHCTR c5
        CTOS

        LDREF       ; load prev action cell
        LDREF       ; load msg cell
        LDU 32      ; load tag
        LDU 8       ; load mode
        ENDS

        PUSHINT 255
        EQUAL
        THROWIFNOT 100

        PUSHINT {tag}
        EQUAL
        THROWIFNOT 100

        ; parse msg
        CTOS
        LDU 32          ; load int from fake msg
        ENDS
        PUSHINT 67890
        EQUAL
        THROWIFNOT 100

        ; check tag and mode in prev action
        CTOS
        LDREF       ; load prev action cell
        LDREF       ; load msg cell
        LDU 32      ; load tag
        LDU 8       ; load mode
        ENDS

        PUSHINT 99
        EQUAL
        THROWIFNOT 100

        PUSHINT {tag}
        EQUAL
        THROWIFNOT 100

        CTOS
        LDU 32
        ENDS
        PUSHINT 12345
        EQUAL
        THROWIFNOT 100

        CTOS
        SEMPTY
        THROWIFNOT 100
        ",
        tag = ACTION_SEND_MSG //4.4.11 blockchain spec
    ))
    .expect_success();
}

#[test]
fn test_send_msg() {
    // 4(5) cells and 8 + 128 + 268 + 8 = 412(412 + 5)
    let mut init = StateInit::with_code_and_data(
        compile_code_to_cell("PUSHINT 1").unwrap(),
        BuilderData::with_raw([1; 128], 128).unwrap().into_cell().unwrap(),
    );
    let code = compile_code_to_cell("PUSHINT 2").unwrap();
    init.set_library_code(code, true).unwrap();

    let root = init.write_to_new_cell().unwrap();
    assert_eq!(root.references_used(), 3);
    assert_eq!(root.bits_used(), 5);
    assert_eq!(root.references()[0].bit_length(), 8);
    assert_eq!(root.references()[0].repr_depth(), 0);
    assert_eq!(root.references()[1].bit_length(), 128);
    assert_eq!(root.references()[1].repr_depth(), 0);
    assert_eq!(root.references()[2].bit_length(), 268);
    assert_eq!(root.references()[2].repr_depth(), 1); // 8 bits in single ref

    // init, body maybe either either - 3 bits
    // external empty header - 102 bits
    // body max - 913 bits (1023 - 3 - 102 - 5)
    // external header - 367 bits
    // body max - 648 bits
    for len in
        [202, 300, 400, 500, 600, 648, 649, 653, 654, 700, 800, 900, 913, 914, 918, 919, 1000, 1023]
    {
        let fee = if len <= 648 {
            // body and init with hdr
            1_000_000 + 400_000 + 412_000 // lump + 4 cells + 412 bits
        } else if len <= 653 {
            // body with hdr init in ref
            1_000_000 + 500_000 + 417_000
        } else if len <= 913 {
            // base: body and init with hdr
            // body in ref
            // init in ref because of bug (must be with hdr)
            1_000_000 + 600_000 + 417_000 + len * 1000
        } else if len <= 918 {
            // base: body with hdr init in ref
            // body in ref
            // init in ref because it was in ref in cell
            1_000_000 + 600_000 + 417_000 + len * 1000
        } else {
            // body in ref init with hdr
            1_000_000 + 500_000 + 412_000 + len * 1000
        };
        let mut msg = Message::with_ext_out_header(Default::default());
        let body = SliceData::from_raw([0; 128], len as usize);
        msg.set_body(body);
        msg.set_state_init(init.clone());
        let out_msg_cell = msg.serialize().unwrap();
        // let (builder, body_to_ref, init_to_ref) = msg.serialize_as_is().unwrap();
        // let out_msg_cell = builder.into_cell().unwrap();
        // println!("len: {len}, body_to_ref: {body_to_ref}, init_to_ref: {init_to_ref}");
        if len > 913 && len <= 918 {
            // cpp bug representation:
            // base body and init with hdr
            // but then body and init in refs (but ref could be with hdr)
            // old algorithm try to calculate storage used for serialization as is
            // then put init to ref if it has at least two refs
            // then put body to ref if it has at least two refs
            // new algorithm in SENDMSG primitive doesn't try to calculate as is
            // it put init to ref if body and init don't fit in hdr
            msg.set_src_address(Default::default());
            let body = msg.body().unwrap().clone().into_cell().unwrap();
            let init = msg.state_init().unwrap().clone().serialize().unwrap();
            let (_, body_to_ref, init_to_ref) = msg.serialize_as_is().unwrap();
            let mut sstat = StorageUsageCalc::with_limits(0, 0);
            sstat.append_cell(&body, body_to_ref, &mut 0).unwrap();
            sstat.append_cell(&init, init_to_ref, &mut 0).unwrap();
            assert_eq!(sstat.cells(), 5);
            assert_eq!(sstat.bits(), 412 + len as u64);
        }
        let code = format!(
            "
            PUSHINT {}
            PUSHREF
            PUSHINT 0
            SENDMSG
        ",
            len
        );
        test_case_with_ref(&code, out_msg_cell)
            .with_mc_state(MC_STATE_ROOT.clone())
            .with_account(SHARD_ACCOUNT.clone())
            .expect_int_stack(&[len, fee]);
    }
    let src = MsgAddressInt::standard(0, [0x11; 32]);
    let dst = MsgAddressInt::standard(0, [0x22; 32]);
    let h = InternalMessageHeader::with_addresses(src, dst, CurrencyCollection::with_coins(6789));
    let msg = Message::with_int_header(h);
    let msg_cell = msg.serialize().unwrap();
    for len in [201, 501, 921, 1023] {
        let fee = if len < 320 { 1812000 } else { 2017000 + len * 1000 };
        let mut value = CurrencyCollection::with_coins(357);
        value.set_other(11, 123).unwrap();
        let h = InternalMessageHeader {
            dst: MsgAddressInt::standard(0, [0x22; 32]),
            ihr_disabled: true,
            value,
            ..Default::default()
        };
        let body = SliceData::from_raw([0; 128], len as usize);
        let mut msg = Message::with_int_header_and_body(h, body);
        msg.set_state_init(init.clone());
        let out_msg_cell = msg.serialize().unwrap();
        let code = format!(
            "
            BLKDROP 5 
            PUSHINT {}
            PUSHREF
            DUP
            DUP
            PUSHINT 0
            SENDMSG
            SWAP
            PUSHINT 64
            SENDMSG
            ROT
            PUSHINT 128
            SENDMSG
        ",
            len
        );
        test_case_with_ref(&code, out_msg_cell)
            .with_mc_state(MC_STATE_ROOT.clone())
            .with_account(SHARD_ACCOUNT.clone())
            .with_message_cell(msg_cell.clone())
            .expect_int_stack(&[len, fee, fee, fee]);
    }
}

#[test]
fn test_send_msg_with_same_cells() {
    let mut params = Vec::new();

    // body cell is present in state_init both are not in refs
    let body = BuilderData::with_raw(vec![1, 2, 3], 24).unwrap().into_cell().unwrap();
    let init =
        StateInit::with_code_and_data(compile_code_to_cell("PUSHINT 1").unwrap(), body.clone());
    params.push((body, init, 892, 1232000));

    // body has same cell as state_init
    let init = StateInit::with_code_and_data(
        compile_code_to_cell("PUSHINT 1").unwrap(),
        BuilderData::with_raw(vec![1, 2, 3], 24).unwrap().into_cell().unwrap(),
    );
    let cell = init.serialize().unwrap();
    let body =
        BuilderData::with_raw_and_refs(vec![1, 2, 3], 24, [cell]).unwrap().into_cell().unwrap();
    params.push((body, init, 992, 1337000));

    for (body, init, gas, expected_fee) in params {
        let src = MsgAddressInt::standard(0, [0x11; 32]);
        let dst = MsgAddressInt::standard(0, [0x22; 32]);
        let h =
            InternalMessageHeader::with_addresses(src, dst, CurrencyCollection::with_coins(6789));
        let body = SliceData::load_cell(body).unwrap();
        let mut msg = Message::with_int_header_and_body(h, body);
        msg.set_state_init(init);
        test_case_with_ref("PUSHREF ZERO SENDMSG", msg.serialize().unwrap())
            .with_mc_state(MC_STATE_ROOT.clone())
            .with_account(SHARD_ACCOUNT.clone())
            .expect_gas_used(gas)
            .expect_int_stack(&[expected_fee]);
    }
}

#[test]
fn test_send_msg_inline_vs_ref_body_gas_accounting() {
    use ton_block::Deserializable;

    // Header has empty src (1 bit) — body fits inline. After SENDMSG sets
    // src to my_addr (~268 bits), the envelope no longer fits, and
    // recalc_serialization_params decides to put body in a ref. This is
    // exactly the layout transition that triggered the original bug:
    //   parsed body_to_ref = Some(false) (inline in source cell)
    //   recalc body_to_ref = true        (body must go to a ref now)
    // Our fix uses parsed flag, not recalc, to decide whether the body
    // cell is real (charge gas) or synthesized (don't charge gas).
    let dst = MsgAddressInt::standard(0, [0x22; 32]);
    // src = AddrNone (~2 bits) so that the source envelope is small enough
    // to keep a moderately-sized body inline. SENDMSG later replaces src
    // with my_addr (AddrStd, ~268 bits) which makes the envelope overflow
    // and forces body into a ref via recalc_serialization_params.
    let h = InternalMessageHeader {
        ihr_disabled: true,
        src: ton_block::MsgAddressIntOrNone::None,
        dst,
        value: CurrencyCollection::with_coins(6789),
        ..Default::default()
    };

    // ~400-bit body: fits inline with empty-src header but must move to ref
    // after SENDMSG sets src to a 267-bit AddrStd.
    let body_inline_then_recalc_ref = SliceData::from_raw(vec![0xAA; 64], 500);
    let msg_a = Message::with_int_header_and_body(h.clone(), body_inline_then_recalc_ref);
    let cell_a = msg_a.serialize().unwrap();
    let parsed_a = Message::construct_from_cell(cell_a.clone()).unwrap();
    assert_eq!(
        parsed_a.body_to_ref(),
        Some(false),
        "test setup: body must be inline in source envelope",
    );

    // Same logical message but with body forcibly placed in a ref by using
    // a body so large that even the empty-src envelope can't hold it inline.
    let body_always_ref = SliceData::from_raw(vec![0xAA; 128], 1000);
    let msg_b = Message::with_int_header_and_body(h, body_always_ref);
    let cell_b = msg_b.serialize().unwrap();
    let parsed_b = Message::construct_from_cell(cell_b.clone()).unwrap();
    assert_eq!(
        parsed_b.body_to_ref(),
        Some(true),
        "test setup: body must already be in ref in source envelope",
    );

    // Inline body in source → SENDMSG must NOT charge root-cell load gas
    // for the body cell, even though recalc puts the body in a ref.
    test_case_with_ref("PUSHREF ZERO SENDMSG", cell_a)
        .with_mc_state(MC_STATE_ROOT.clone())
        .with_account(SHARD_ACCOUNT.clone())
        .expect_gas_used(692);

    // Body originally in ref → SENDMSG must charge a root-cell load.
    test_case_with_ref("PUSHREF ZERO SENDMSG", cell_b)
        .with_mc_state(MC_STATE_ROOT.clone())
        .with_account(SHARD_ACCOUNT.clone())
        .expect_gas_used(792);
}

#[test]
fn test_rawreserve_with_parsing() {
    let reserved_coins = 123456789u128;
    let flags = 3u8;
    let mut out_actions = BuilderData::new();
    out_actions
        .append_u32(ACTION_RESERVE)
        .and_then(|b| b.append_u8(flags))
        .and_then(|b| {
            b.append_builder(&serialize_currency_collection(reserved_coins, None).unwrap())
        })
        .unwrap();
    out_actions.checked_append_reference(Cell::default()).unwrap();

    test_case(format!(
        "
        PUSHINT {}
        PUSHINT {}
        RAWRESERVE
        PUSHCTR c5
        ",
        reserved_coins, flags
    ))
    .expect_item(StackItem::Cell(out_actions.into_cell().unwrap()));
}

#[test]
fn test_rawreservex_with_parsing() {
    let reserved_coins = 123456789u128;
    let mut other = HashmapE::with_bit_len(32);
    let key = BuilderData::new().append_u32(1).unwrap().clone();
    let value = BuilderData::new().append_u128(0).unwrap().append_u128(100).unwrap().clone();
    other.set_builder(SliceData::load_builder(key).unwrap(), &value).unwrap();
    let key = BuilderData::new().append_u32(2).unwrap().clone();
    let value = BuilderData::new().append_u128(0).unwrap().append_u128(200).unwrap().clone();
    other.set_builder(SliceData::load_builder(key).unwrap(), &value).unwrap();
    let currency = &serialize_currency_collection(reserved_coins, other.data().cloned()).unwrap();
    let flags = 3u8;
    let mut out_actions = BuilderData::new();
    out_actions.checked_append_reference(Cell::default()).unwrap();
    out_actions
        .append_u32(ACTION_RESERVE)
        .and_then(|b| b.append_u8(flags))
        .and_then(|b| b.append_builder(currency))
        .unwrap();

    test_case_with_ref(
        &format!(
            "
        PUSHINT {}
        PUSHREF
        PUSHINT {}
        RAWRESERVEX
        PUSHCTR c5
        ",
            reserved_coins, flags
        ),
        currency.references()[0].clone(),
    )
    .expect_item(StackItem::Cell(out_actions.into_cell().unwrap()));
}

#[test]
fn test_setcode_with_parsing() {
    let code = compile_code_to_cell("PUSHINT 1").unwrap();
    let mut out_actions = BuilderData::new();
    out_actions.append_u32(ACTION_SET_CODE).unwrap();
    out_actions.checked_append_reference(Cell::default()).unwrap();
    out_actions.checked_append_reference(code.clone()).unwrap();

    test_case(
        "
        PUSHREF
        SETCODE
        PUSHCTR c5
    ",
    )
    .with_ref(code)
    .expect_item(StackItem::Cell(out_actions.into_cell().unwrap()));
}

#[test]
fn test_setlibcode_with_parsing() {
    let code = compile_code_to_cell("PUSHINT 1").unwrap();
    let mut out_actions = BuilderData::new();
    out_actions.append_u32(ACTION_CHANGE_LIB).unwrap();
    out_actions.append_u8(3).unwrap();
    out_actions.checked_append_reference(Cell::default()).unwrap();
    out_actions.checked_append_reference(code.clone()).unwrap();

    test_case(
        "PUSHREF
        PUSHINT 1
        SETLIBCODE
        PUSHCTR c5",
    )
    .with_ref(code)
    .expect_item(StackItem::Cell(out_actions.into_cell().unwrap()));
}

#[test]
fn test_changelib_with_parsing() {
    let code = compile_code_to_cell("PUSHINT 1").unwrap();
    let hash = code.repr_hash();
    let mut out_actions = BuilderData::new();
    out_actions.append_u32(ACTION_CHANGE_LIB).unwrap();
    out_actions.append_u8(2).unwrap();
    out_actions.append_raw(hash.as_slice(), 256).unwrap();
    out_actions.checked_append_reference(Cell::default()).unwrap();

    test_case(
        "PUSHREF
        HASHCU
        PUSHINT 1
        CHANGELIB
        PUSHCTR c5",
    )
    .with_ref(code)
    .expect_item(StackItem::Cell(out_actions.into_cell().unwrap()));
}

#[test]
fn test_changelib_errors() {
    expect_exception("SETLIBCODE", ExceptionCode::StackUnderflow);
    expect_exception("ZERO SETLIBCODE", ExceptionCode::StackUnderflow);
    expect_exception("NULL ZERO SETLIBCODE", ExceptionCode::TypeCheckError);
    expect_exception("NEWC ENDC TEN SETLIBCODE", ExceptionCode::RangeCheckError);
    expect_exception("CHANGELIB", ExceptionCode::StackUnderflow);
    expect_exception("ZERO CHANGELIB", ExceptionCode::StackUnderflow);
    expect_exception("NULL ZERO CHANGELIB", ExceptionCode::TypeCheckError);
    expect_exception("ZERO TEN CHANGELIB", ExceptionCode::RangeCheckError);
    expect_exception("PUSHINT -1 ZERO CHANGELIB", ExceptionCode::RangeCheckError);
    expect_exception("PUSHNEGPOW2 256 ZERO CHANGELIB", ExceptionCode::RangeCheckError);
}

#[test]
fn test_rawreserve_stackunderflow() {
    expect_exception("SETCODE", ExceptionCode::StackUnderflow);
    expect_exception("NULL SETCODE", ExceptionCode::TypeCheckError);
    expect_exception("ZERO SETCODE", ExceptionCode::TypeCheckError);
    expect_exception("RAWRESERVE", ExceptionCode::StackUnderflow);
    expect_exception("PUSHINT 0 RAWRESERVE", ExceptionCode::StackUnderflow);
    expect_exception("RAWRESERVEX", ExceptionCode::StackUnderflow);
    expect_exception("PUSHINT 0 RAWRESERVEX", ExceptionCode::StackUnderflow);
    expect_exception("NEWC ENDC CTOS RAWRESERVEX", ExceptionCode::StackUnderflow);
}

#[test]
fn test_rawreserve_range_check_err() {
    expect_exception(
        "PUSHINT 10
        PUSHINT 32
        RAWRESERVE",
        ExceptionCode::RangeCheckError,
    );

    expect_exception(
        "PUSHINT 10
        PUSHINT -1
        RAWRESERVE",
        ExceptionCode::RangeCheckError,
    );

    expect_exception(
        "PUSHINT -1
        PUSHINT 0
        RAWRESERVE",
        ExceptionCode::RangeCheckError,
    );

    expect_exception(
        "PUSHINT 10
        NULL
        PUSHINT 32
        RAWRESERVEX",
        ExceptionCode::RangeCheckError,
    );

    expect_exception(
        "PUSHINT -1
        NULL
        PUSHINT 1
        RAWRESERVEX",
        ExceptionCode::RangeCheckError,
    );
}

#[test]
fn test_rawreserve_type_check_err() {
    expect_exception(
        "PUSHSLICE x8_
        PUSHINT 0
        RAWRESERVE",
        ExceptionCode::TypeCheckError,
    );

    expect_exception(
        "PUSHINT 0
        PUSHSLICE x8_
        RAWRESERVE",
        ExceptionCode::TypeCheckError,
    );

    expect_exception(
        "PUSHSLICE x8_
        NULL
        PUSHSLICE x8_
        RAWRESERVEX",
        ExceptionCode::TypeCheckError,
    );

    expect_exception(
        "PUSHINT 0
        PUSHINT 0
        PUSHINT 0
        RAWRESERVEX",
        ExceptionCode::TypeCheckError,
    );
}

fn write_msg_adress(tuple: &[StackItem]) -> Result<BuilderData> {
    let addr_type = tuple[0].as_integer_value(0..=3u8)?;
    let mut cell = BuilderData::with_raw(vec![addr_type << 6], 2)?;
    match addr_type {
        0b00 => (),
        0b01 => {
            let address = tuple[1].as_slice()?;
            let bits = address.remaining_bits();
            cell.append_bits(bits, 9)?;
            cell.append_bytestring(address)?;
        }
        0b10 => {
            match &tuple[1] {
                StackItem::Slice(rewrite_pfx) => {
                    cell.append_bit_one()?;
                    let bits = rewrite_pfx.remaining_bits();
                    cell.append_bits(bits, 5)?;
                    cell.append_bytestring(rewrite_pfx)?;
                }
                StackItem::None => {
                    cell.append_bit_zero()?;
                }
                _ => unreachable!(),
            }
            let workchain_id = tuple[2].as_integer_value(-128i8..=127i8)?;
            cell.append_i8(workchain_id)?;
            cell.append_bytestring(tuple[3].as_slice()?)?;
        }
        0b11 => {
            match &tuple[1] {
                StackItem::Slice(rewrite_pfx) => {
                    cell.append_bit_one()?;
                    let bits = rewrite_pfx.remaining_bits();
                    cell.append_bits(bits, 5)?;
                    cell.append_bytestring(rewrite_pfx)?;
                }
                StackItem::None => {
                    cell.append_bit_zero()?;
                }
                _ => unreachable!(),
            }
            let address = tuple[3].as_slice()?;
            let bits = address.remaining_bits();
            cell.append_bits(bits, 9)?;
            let workchain_id = tuple[2].as_integer_value(i32::MIN..=i32::MAX)?;
            cell.append_i32(workchain_id)?;
            cell.append_bytestring(address)?;
        }
        _ => unreachable!(),
    }
    Ok(cell)
}

fn check_msg_adr_bad(tuple: Vec<StackItem>) {
    let mut cell = write_msg_adress(&tuple).unwrap();
    let slice = SliceData::load_builder(cell.clone()).unwrap();
    let remainder = [0xEE, 0xAB, 0x80];
    cell.append_raw(&remainder, 16).unwrap();
    let cell = cell.into_cell().unwrap();

    test_case_with_ref(
        "
        PUSHREFSLICE
        LDMSGADDR
    ",
        cell.clone(),
    )
    .expect_failure(ExceptionCode::CellUnderflow);

    test_case_with_ref(
        "
        PUSHREFSLICE
        LDMSGADDRQ
    ",
        cell.clone(),
    )
    .expect_stack(
        Stack::new()
            .push(StackItem::Slice(SliceData::load_cell(cell).unwrap()))
            // .push(create::slice(remainder))
            .push(boolean!(false)),
    );

    test_case_with_ref(
        "
        PUSHREFSLICE
        PARSEMSGADDR
    ",
        slice.clone().into_cell().unwrap(),
    )
    .expect_failure(ExceptionCode::CellUnderflow);

    test_case_with_ref(
        "
        PUSHREFSLICE
        PARSEMSGADDRQ
    ",
        slice.clone().into_cell().unwrap(),
    )
    .expect_stack(
        Stack::new()
            // .push(create::tuple(&tuple))
            .push(boolean!(false)),
    );

    test_case_with_ref(
        "
        PUSHREFSLICE
        REWRITESTDADDR
        NEWC
        STU 256
        ENDC
        CTOS
    ",
        slice.clone().into_cell().unwrap(),
    )
    .expect_failure(ExceptionCode::CellUnderflow);

    test_case_with_ref(
        "
        PUSHREFSLICE
        REWRITESTDADDR
    ",
        slice.clone().into_cell().unwrap(),
    )
    .expect_failure(ExceptionCode::CellUnderflow);

    test_case_with_ref(
        "
        PUSHREFSLICE
        REWRITESTDADDRQ
    ",
        slice.clone().into_cell().unwrap(),
    )
    .expect_item(boolean!(false));

    test_case_with_ref(
        "
        PUSHREFSLICE
        REWRITEVARADDR
    ",
        slice.clone().into_cell().unwrap(),
    )
    .expect_failure(ExceptionCode::CellUnderflow);

    test_case_with_ref(
        "
        PUSHREFSLICE
        REWRITEVARADDRQ
    ",
        slice.into_cell().unwrap(),
    )
    .expect_item(boolean!(false));
}

fn check_msg_adr(tuple: Vec<StackItem>, rewrite: Option<SliceData>) {
    let mut cell = write_msg_adress(&tuple).unwrap();
    let slice = SliceData::load_builder(cell.clone()).unwrap();
    let remainder = [0xEE, 0xAB, 0x80];
    cell.append_raw(&remainder, 16).unwrap();
    let cell = cell.into_cell().unwrap();

    test_case_with_ref(
        "
        PUSHREFSLICE
        LDMSGADDR
    ",
        cell.clone(),
    )
    .expect_stack(
        Stack::new().push(StackItem::Slice(slice.clone())).push(create::slice(remainder)),
    );

    test_case_with_ref(
        "
        PUSHREFSLICE
        LDMSGADDRQ
    ",
        cell,
    )
    .expect_stack(
        Stack::new()
            .push(StackItem::Slice(slice.clone()))
            .push(create::slice(remainder))
            .push(boolean!(true)),
    );

    test_case_with_ref(
        "
        PUSHREFSLICE
        PARSEMSGADDR
    ",
        slice.clone().into_cell().unwrap(),
    )
    .expect_stack(Stack::new().push(create::tuple(&tuple)));

    test_case_with_ref(
        "
        PUSHREFSLICE
        PARSEMSGADDRQ
    ",
        slice.clone().into_cell().unwrap(),
    )
    .expect_stack(Stack::new().push(create::tuple(&tuple)).push(boolean!(true)));

    let execution_result = test_case_with_ref(
        "
        PUSHREFSLICE
        REWRITESTDADDR
        NEWC
        STU 256
        ENDC
        CTOS
    ",
        slice.clone().into_cell().unwrap(),
    );
    let _ = match rewrite {
        None => execution_result.expect_failure(ExceptionCode::CellUnderflow),
        Some(ref rewrite) => {
            if rewrite.remaining_bits() == 256 {
                execution_result.expect_stack(
                    Stack::new().push(tuple[2].clone()).push(StackItem::Slice(rewrite.clone())),
                )
            } else {
                execution_result.expect_failure(ExceptionCode::CellUnderflow)
            }
        }
    };

    let execution_result = test_case_with_ref(
        "
        PUSHREFSLICE
        REWRITESTDADDR
    ",
        slice.clone().into_cell().unwrap(),
    );
    let _ = match rewrite {
        None => execution_result.expect_failure(ExceptionCode::CellUnderflow),
        Some(ref rewrite) => {
            if rewrite.remaining_bits() == 256 {
                execution_result.expect_stack(Stack::new().push(tuple[2].clone()).push(
                    StackItem::integer(IntegerData::from_unsigned_bytes_be(
                        rewrite.get_bytestring(0),
                    )),
                ))
            } else {
                execution_result.expect_failure(ExceptionCode::CellUnderflow)
            }
        }
    };

    let execution_result = test_case_with_ref(
        "
        PUSHREFSLICE
        REWRITESTDADDRQ
    ",
        slice.clone().into_cell().unwrap(),
    );
    let _ = match rewrite {
        None => execution_result.expect_item(boolean!(false)),
        Some(ref rewrite) => {
            if rewrite.remaining_bits() == 256 {
                execution_result.expect_stack(
                    Stack::new()
                        .push(tuple[2].clone())
                        .push(StackItem::integer(IntegerData::from_unsigned_bytes_be(
                            rewrite.get_bytestring(0),
                        )))
                        .push(boolean!(true)),
                )
            } else {
                execution_result.expect_item(boolean!(false))
            }
        }
    };

    let execution_result = test_case_with_ref(
        "
        PUSHREFSLICE
        REWRITEVARADDR
    ",
        slice.clone().into_cell().unwrap(),
    );
    let _ = match rewrite {
        None => execution_result.expect_failure(ExceptionCode::CellUnderflow),
        Some(ref rewrite) => execution_result.expect_stack(
            Stack::new().push(tuple[2].clone()).push(StackItem::Slice(rewrite.clone())),
        ),
    };

    let execution_result = test_case_with_ref(
        "
        PUSHREFSLICE
        REWRITEVARADDRQ
    ",
        slice.into_cell().unwrap(),
    );
    let _ = match rewrite {
        None => execution_result.expect_item(boolean!(false)),
        Some(ref rewrite) => execution_result.expect_stack(
            Stack::new()
                .push(tuple[2].clone())
                .push(StackItem::Slice(rewrite.clone()))
                .push(boolean!(true)),
        ),
    };
}

#[test]
fn test_load_msg_addr_normal() {
    let acc_addr = SliceData::from_raw(vec![0x11; 32], 256);

    let acc_slice = StackItem::Slice(acc_addr.clone());

    // let addr = MsgAddressInt::AddrNone;
    check_msg_adr(vec![int!(0)], None);

    // let addr = MsgAddressExt::with_extern(acc_addr.clone()).unwrap();
    check_msg_adr(vec![int!(1), acc_slice.clone()], None);

    // Standart without prefix
    // let addr = MsgAddressInt::with_standart(None, 1, acc_addr.clone()).unwrap();
    check_msg_adr(
        vec![int!(2), StackItem::None, int!(1), acc_slice.clone()],
        Some(acc_addr.clone()),
    );
}

#[test]
fn test_load_msg_addr_bad() {
    let short_addr = SliceData::new(vec![0x11, 0x22, 0x80]);
    let long_addr = SliceData::new(vec![0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x80]);
    let acc_addr = SliceData::from_raw(vec![0x11; 32], 256);
    let prefix = SliceData::new(vec![0x77, 0x88, 0x99, 0x80]);

    let prefix_slice = StackItem::Slice(prefix);
    let short_slice = StackItem::Slice(short_addr.clone());
    let long_slice = StackItem::Slice(long_addr.clone());
    let acc_slice = StackItem::Slice(acc_addr.clone());

    // let rewrite_pfx  = Some(AnycastInfo::with_rewrite_pfx(prefix));

    // Standart with prefix
    // let addr = MsgAddressInt::with_standart(rewrite_pfx.clone(), 2, acc_addr.clone()).unwrap();
    check_msg_adr_bad(vec![int!(2), prefix_slice.clone(), int!(2), acc_slice.clone()]);

    // Variant with 256 bit addr and without prefix
    // let addr = MsgAddressInt::with_variant(None, 3, acc_addr.clone()).unwrap();
    check_msg_adr_bad(vec![int!(3), StackItem::None, int!(3), acc_slice.clone()]);

    // Variant with 256 bit addr and prefix
    // let addr = MsgAddressInt::with_variant(rewrite_pfx.clone(), 4, acc_addr.clone()).unwrap();
    check_msg_adr_bad(vec![int!(3), prefix_slice.clone(), int!(4), acc_slice]);

    // Variant with short addr and without prefix
    // let addr = MsgAddressInt::with_variant(None, 5, short_addr.clone()).unwrap();
    check_msg_adr_bad(vec![int!(3), StackItem::None, int!(5), short_slice.clone()]);

    // Variant with addr shorter than prefix
    // let addr = MsgAddressInt::with_variant(rewrite_pfx.clone(), 6, short_addr.clone()).unwrap();
    check_msg_adr_bad(vec![int!(3), prefix_slice.clone(), int!(6), short_slice]);

    // Variant with long addr and without prefix
    // let addr = MsgAddressInt::with_variant(None, 7, long_addr.clone()).unwrap();
    check_msg_adr_bad(vec![int!(3), StackItem::None, int!(7), long_slice.clone()]);

    // Variant with addr longer than prefix
    // let addr = MsgAddressInt::with_variant(rewrite_pfx.clone(), 8, long_addr.clone()).unwrap();
    check_msg_adr_bad(vec![int!(3), prefix_slice, int!(8), long_slice]);
}

#[test]
fn test_load_msg_addr_with_error() {
    test_case(
        "
        PUSHSLICE xE_
        LDMSGADDR
    ",
    )
    .expect_failure(ExceptionCode::CellUnderflow);

    test_case(
        "
        LDMSGADDR
    ",
    )
    .expect_failure(ExceptionCode::StackUnderflow);

    test_case(
        "
        PUSHSLICE xE_
        LDMSGADDRQ
    ",
    )
    .expect_stack(Stack::new().push(create::slice([0xE0])).push(boolean!(false)));
}

#[test]
fn test_parse_msg_addr_with_error() {
    test_case(
        "
        PUSHSLICE xE_
        PARSEMSGADDR
    ",
    )
    .expect_failure(ExceptionCode::CellUnderflow);

    test_case(
        "
        PUSHSLICE xE_
        PARSEMSGADDRQ
    ",
    )
    .expect_stack(Stack::new().push(boolean!(false)));
}

#[test]
fn test_load_opt_std_address() {
    let addr = MsgAddressInt::standard(0, [0x11; 32]);
    let slice = addr.write_to_bitstring().unwrap();
    test_case_with_ref(
        "
        PUSHREFSLICE
        LDSTDADDR
    ",
        slice.clone().into_cell().unwrap(),
    )
    .expect_stack(Stack::new().push_slice(slice.clone()).push_slice(SliceData::new_empty()));

    test_case_with_ref(
        "
        PUSHREFSLICE
        LDOPTSTDADDR
    ",
        slice.clone().into_cell().unwrap(),
    )
    .expect_stack(Stack::new().push_slice(slice.clone()).push_slice(SliceData::new_empty()));

    test_case_with_ref(
        "
        PUSHREFSLICE
        LDSTDADDRQ
    ",
        slice.clone().into_cell().unwrap(),
    )
    .expect_stack(
        Stack::new().push_slice(slice.clone()).push_slice(SliceData::new_empty()).push_bool(true),
    );

    test_case_with_ref(
        "
        PUSHREFSLICE
        LDOPTSTDADDRQ
    ",
        slice.clone().into_cell().unwrap(),
    )
    .expect_stack(
        Stack::new().push_slice(slice.clone()).push_slice(SliceData::new_empty()).push_bool(true),
    );

    let addr = MsgAddress::AddrNone;
    let slice = addr.write_to_bitstring().unwrap();
    test_case_with_ref(
        "
        PUSHREFSLICE
        LDOPTSTDADDR
    ",
        slice.clone().into_cell().unwrap(),
    )
    .expect_stack(Stack::new().push_null().push_slice(SliceData::new_empty()));

    test_case_with_ref(
        "
        PUSHREFSLICE
        LDOPTSTDADDRQ
    ",
        slice.into_cell().unwrap(),
    )
    .expect_stack(Stack::new().push_null().push_slice(SliceData::new_empty()).push_bool(true));

    let prefix = AnycastInfo::with_rewrite_pfx(SliceData::new(vec![0x35, 0x80])).unwrap();
    let addr = MsgAddressInt::with_standart(Some(prefix.clone()), 0, [0x11; 32].into()).unwrap();
    let slice = addr.write_to_bitstring().unwrap();
    test_case_with_ref(
        "
        PUSHREFSLICE
        LDSTDADDR
    ",
        slice.clone().into_cell().unwrap(),
    )
    .expect_failure(ExceptionCode::CellUnderflow);

    test_case_with_ref(
        "
        PUSHREFSLICE
        LDSTDADDRQ
    ",
        slice.clone().into_cell().unwrap(),
    )
    .expect_stack(Stack::new().push_slice(slice.clone()).push_bool(false));

    test_case_with_ref(
        "
        PUSHREFSLICE
        LDOPTSTDADDR
    ",
        slice.clone().into_cell().unwrap(),
    )
    .expect_failure(ExceptionCode::CellUnderflow);

    test_case_with_ref(
        "
        PUSHREFSLICE
        LDOPTSTDADDRQ
    ",
        slice.clone().into_cell().unwrap(),
    )
    .expect_stack(Stack::new().push_slice(slice.clone()).push_bool(false));

    let addr = MsgAddressInt::with_variant(Some(prefix), 1234, [0x11; 32].into()).unwrap();
    let slice = addr.write_to_bitstring().unwrap();
    test_case_with_ref(
        "
        PUSHREFSLICE
        LDSTDADDR
    ",
        slice.clone().into_cell().unwrap(),
    )
    .expect_failure(ExceptionCode::CellUnderflow);

    test_case_with_ref(
        "
        PUSHREFSLICE
        LDSTDADDRQ
    ",
        slice.clone().into_cell().unwrap(),
    )
    .expect_stack(Stack::new().push_slice(slice.clone()).push_bool(false));

    test_case_with_ref(
        "
        PUSHREFSLICE
        LDOPTSTDADDR
    ",
        slice.clone().into_cell().unwrap(),
    )
    .expect_failure(ExceptionCode::CellUnderflow);

    test_case_with_ref(
        "
        PUSHREFSLICE
        LDOPTSTDADDRQ
    ",
        slice.clone().into_cell().unwrap(),
    )
    .expect_stack(Stack::new().push_slice(slice.clone()).push_bool(false));

    expect_exception("LDSTDADDR", ExceptionCode::StackUnderflow);
    expect_exception("ONE LDSTDADDR", ExceptionCode::TypeCheckError);
    expect_exception("LDSTDADDRQ", ExceptionCode::StackUnderflow);
    expect_exception("ONE LDSTDADDRQ", ExceptionCode::TypeCheckError);
    expect_exception("LDOPTSTDADDR", ExceptionCode::StackUnderflow);
    expect_exception("ONE LDOPTSTDADDR", ExceptionCode::TypeCheckError);
    expect_exception("LDOPTSTDADDRQ", ExceptionCode::StackUnderflow);
    expect_exception("ONE LDOPTSTDADDRQ", ExceptionCode::TypeCheckError);
}

#[test]
fn test_store_opt_std_address() {
    expect_exception("NULL NEWC PUSHINT 1022 STONES STOPTSTDADDR", ExceptionCode::CellOverflow);

    expect_exception("STSTDADDR", ExceptionCode::StackUnderflow);
    expect_exception("NEWC STSTDADDR", ExceptionCode::StackUnderflow);
    expect_exception("PUSHSLICE x2_ NEWC STSTDADDR", ExceptionCode::CellOverflow);
    expect_exception("ZERO NEWC STSTDADDR", ExceptionCode::TypeCheckError);
    expect_exception("NEWC NEWC STSTDADDR", ExceptionCode::TypeCheckError);
    expect_exception("PUSHSLICE x2_ ZERO STSTDADDR", ExceptionCode::TypeCheckError);

    expect_exception("STSTDADDRQ", ExceptionCode::StackUnderflow);
    expect_exception("NEWC STSTDADDRQ", ExceptionCode::StackUnderflow);
    expect_exception("ZERO NEWC STSTDADDRQ", ExceptionCode::TypeCheckError);
    expect_exception("NEWC NEWC STSTDADDRQ", ExceptionCode::TypeCheckError);
    expect_exception("PUSHSLICE x2_ ZERO STSTDADDRQ", ExceptionCode::TypeCheckError);

    expect_exception("STOPTSTDADDR", ExceptionCode::StackUnderflow);
    expect_exception("NEWC STOPTSTDADDR", ExceptionCode::StackUnderflow);
    expect_exception("PUSHSLICE x2_ NEWC STOPTSTDADDR", ExceptionCode::CellOverflow);
    expect_exception("NEWC NEWC STOPTSTDADDR", ExceptionCode::TypeCheckError);
    expect_exception("ZERO NEWC STOPTSTDADDR", ExceptionCode::TypeCheckError);
    expect_exception("PUSHSLICE x2_ ZERO STOPTSTDADDR", ExceptionCode::TypeCheckError);

    expect_exception("STOPTSTDADDRQ", ExceptionCode::StackUnderflow);
    expect_exception("NEWC STOPTSTDADDRQ", ExceptionCode::StackUnderflow);
    expect_exception("NEWC ZERO STOPTSTDADDRQ", ExceptionCode::TypeCheckError);
    test_case("NEWC NEWC STOPTSTDADDRQ").expect_stack(
        Stack::new()
            .push_slice(Default::default())
            .push_builder(Default::default())
            .push_bool(true),
    );

    // standard address with anycast
    let anycast = AnycastInfo::with_rewrite_pfx(SliceData::new(vec![0x35, 0x80])).unwrap();
    let addr = MsgAddressInt::with_standart(Some(anycast), 0, [0x11; 32].into()).unwrap();
    let slice = addr.write_to_bitstring().unwrap();
    test_case_with_ref("PUSHREFSLICE NEWC STSTDADDR", slice.clone().into_cell().unwrap())
        .expect_failure(ExceptionCode::CellOverflow);

    test_case_with_ref("PUSHREFSLICE NEWC STSTDADDRQ", slice.clone().into_cell().unwrap())
        .expect_stack(
            Stack::new().push_slice(slice.clone()).push_builder(Default::default()).push_bool(true),
        );

    test_case_with_ref("PUSHREFSLICE NEWC STOPTSTDADDR", slice.clone().into_cell().unwrap())
        .expect_failure(ExceptionCode::CellOverflow);

    test_case_with_ref("PUSHREFSLICE NEWC STOPTSTDADDRQ", slice.clone().into_cell().unwrap())
        .expect_stack(
            Stack::new().push_slice(slice).push_builder(Default::default()).push_bool(true),
        );

    // variant address without anycast
    let addr = MsgAddressInt::with_variant(None, 0, SliceData::new(vec![0x22, 0x80])).unwrap();
    let slice = addr.write_to_bitstring().unwrap();
    test_case_with_ref("PUSHREFSLICE NEWC STSTDADDR", slice.clone().into_cell().unwrap())
        .expect_failure(ExceptionCode::CellOverflow);

    test_case_with_ref("PUSHREFSLICE NEWC STSTDADDRQ", slice.clone().into_cell().unwrap())
        .expect_stack(
            Stack::new().push_slice(slice.clone()).push_builder(Default::default()).push_bool(true),
        );

    test_case_with_ref("PUSHREFSLICE NEWC STOPTSTDADDR", slice.clone().into_cell().unwrap())
        .expect_failure(ExceptionCode::CellOverflow);

    test_case_with_ref("PUSHREFSLICE NEWC STOPTSTDADDRQ", slice.clone().into_cell().unwrap())
        .expect_stack(
            Stack::new().push_slice(slice).push_builder(Default::default()).push_bool(true),
        );

    // standard address without anycast
    let addr = MsgAddressInt::standard(0, [0x33; 32]);
    let builder = addr.write_to_new_cell().unwrap();
    let cell = builder.clone().into_cell().unwrap();
    test_case_with_ref("PUSHREFSLICE NEWC STSTDADDR", cell.clone())
        .expect_stack(Stack::new().push_builder(builder.clone()));

    test_case_with_ref("PUSHREFSLICE NEWC STSTDADDRQ", cell.clone())
        .expect_stack(Stack::new().push_builder(builder.clone()).push_bool(false));

    test_case_with_ref("PUSHREFSLICE NEWC STOPTSTDADDR", cell.clone())
        .expect_stack(Stack::new().push_builder(builder.clone()));

    test_case_with_ref("PUSHREFSLICE NEWC STOPTSTDADDRQ", cell.clone())
        .expect_stack(Stack::new().push_builder(builder.clone()).push_bool(false));

    let builder = BuilderData::with_raw([0xff; 128], 1021).unwrap();
    test_case_with_ref("PUSHREFSLICE NEWC PUSHINT 1021 STONES STSTDADDR", cell.clone())
        .expect_failure(ExceptionCode::CellOverflow);

    test_case_with_ref("PUSHREFSLICE NEWC PUSHINT 1021 STONES STSTDADDRQ", cell.clone())
        .expect_stack(
            Stack::new()
                .push_slice(SliceData::load_cell(cell.clone()).unwrap())
                .push_builder(builder.clone())
                .push_bool(true),
        );

    test_case_with_ref("PUSHREFSLICE NEWC PUSHINT 1021 STONES STOPTSTDADDR", cell.clone())
        .expect_failure(ExceptionCode::CellOverflow);

    test_case_with_ref("PUSHREFSLICE NEWC PUSHINT 1021 STONES STOPTSTDADDRQ", cell.clone())
        .expect_stack(
            Stack::new()
                .push_slice(SliceData::load_cell(cell).unwrap())
                .push_builder(builder)
                .push_bool(true),
        );

    // address none
    let addr = MsgAddress::AddrNone;
    let builder = addr.write_to_new_cell().unwrap();
    test_case("NULL NEWC STSTDADDR").expect_failure(ExceptionCode::TypeCheckError);

    test_case("NULL NEWC STSTDADDRQ").expect_failure(ExceptionCode::TypeCheckError);

    test_case("NULL NEWC STOPTSTDADDR").expect_stack(Stack::new().push_builder(builder.clone()));

    test_case("NULL NEWC STOPTSTDADDRQ")
        .expect_stack(Stack::new().push_builder(builder).push_bool(false));

    let builder = BuilderData::with_raw([0xff; 128], 1022).unwrap();
    test_case("NULL NEWC PUSHINT 1022 STONES STOPTSTDADDRQ")
        .expect_stack(Stack::new().push_null().push_builder(builder).push_bool(true));
}

mod p256 {
    use super::*;

    #[test]
    fn test_check_signature_bad() {
        expect_exception("P256_CHKSIGNS", ExceptionCode::StackUnderflow);
        expect_exception("NULL P256_CHKSIGNS", ExceptionCode::StackUnderflow);
        expect_exception("NULL NULL P256_CHKSIGNS", ExceptionCode::StackUnderflow);
        expect_exception("NULL NULL NULL P256_CHKSIGNS", ExceptionCode::TypeCheckError);

        // data is not byte aligned
        expect_exception("
            PUSHSLICE x48656c6c6f2c20776f726c6421_
            PUSHSLICE x979e54eb9b3a942243effbd39ccdb309f4bd2973e77c31b92d04150c169e9e3597b666d6d2fd084c38e1eb1ed828e11c3e4fbdc74c18b0d466375e4304673be7
            PUSHSLICE x024c0fb7f19c9879471f2eb3c57cc674d52483d8de673c566ccb6c13c17c16f3cd
            P256_CHKSIGNS
        ", ExceptionCode::CellUnderflow);

        // signature less than 64 bytes
        expect_exception("
            PUSHSLICE x48656c6c6f2c20776f726c6421
            PUSHSLICE x979e54eb9b3a942243effbd39ccdb309f4bd2973e77c31b92d04150c169e9e3597b666d6d2fd084c38e1eb1ed828e11c3e4fbdc74c18b0d466375e4304673b
            PUSHSLICE x024c0fb7f19c9879471f2eb3c57cc674d52483d8de673c566ccb6c13c17c16f3cd
            P256_CHKSIGNS
        ", ExceptionCode::CellUnderflow);

        // public key less than 33 bytes
        expect_exception("
            PUSHSLICE x48656c6c6f2c20776f726c6421
            PUSHSLICE x979e54eb9b3a942243effbd39ccdb309f4bd2973e77c31b92d04150c169e9e3597b666d6d2fd084c38e1eb1ed828e11c3e4fbdc74c18b0d466375e4304673be7
            PUSHSLICE x024c0fb7f19c9879471f2eb3c57cc674d52483d8de673c566ccb6c13c17c16f3
            P256_CHKSIGNS
        ", ExceptionCode::CellUnderflow);
    }

    #[test]
    fn test_check_signature_good() {
        // signature length is allowed to be more than 64 bytes
        // public key length is allowed to be more than 33 bytes

        test_case("
            PUSHSLICE x73616d706c65 ; simple
            PUSHSLICE xEFD48B2AACB6A8FD1140DD9CD45E81D69D2C877B56AAF991C34D0EA84EAF3716F7CB1C942D657C41D436C7A1B6E29F65F3E900DBB9AFF4064DC4AB2F843ACDA8
            PUSHSLICE x0360FED4BA255A9D31C961EB74C6356D68C049B8923B61FA6CE669622E60F29FB6
            P256_CHKSIGNS
        ")
        .expect_int_stack(&[-1]);

        test_case("
            PUSHSLICE x74657374 ; test
            PUSHSLICE xF1ABB023518351CD71D881567B1EA663ED3EFCF6C5132B354F28D3B0B7D38367019F4113742A2B14BD25926B49C649155F267E60D3814B4C0CC84250E46F0083
            PUSHSLICE x0360FED4BA255A9D31C961EB74C6356D68C049B8923B61FA6CE669622E60F29FB6
            P256_CHKSIGNS
        ")
        .expect_int_stack(&[-1]);

        test_case("
            PUSHSLICE x74657374 ; test
            PUSHSLICE x83910E8B48BB0C74244EBDF7F07A1C5413D61472BD941EF3920E623FBCCEBEB68DDBEC54CF8CD5874883841D712142A56A8D0F218F5003CB0296B6B509619F2C
            PUSHSLICE x0360FED4BA255A9D31C961EB74C6356D68C049B8923B61FA6CE669622E60F29FB6
            P256_CHKSIGNS
        ")
        .expect_int_stack(&[0]);

        test_case("
            PUSHSLICE x48656c6c6f2c20776f726c6421
            PUSHSLICE x979e54eb9b3a942243effbd39ccdb309f4bd2973e77c31b92d04150c169e9e3597b666d6d2fd084c38e1eb1ed828e11c3e4fbdc74c18b0d466375e4304673be71_
            PUSHSLICE x024c0fb7f19c9879471f2eb3c57cc674d52483d8de673c566ccb6c13c17c16f3cd1_
            P256_CHKSIGNS
        ")
        .expect_int_stack(&[-1]);

        test_case("
            PUSHSLICE x
            PUSHSLICE x979e54eb9b3a942243effbd39ccdb309f4bd2973e77c31b92d04150c169e9e3597b666d6d2fd084c38e1eb1ed828e11c3e4fbdc74c18b0d466375e4304673be7
            PUSHSLICE x024c0fb7f19c9879471f2eb3c57cc674d52483d8de673c566ccb6c13c17c16f3cd
            P256_CHKSIGNS
        ")
        .expect_int_stack(&[0]);

        test_case("
            PUSHINT 22331814027392488307105736075480205742348666473969333634173732071459215699411
            PUSHSLICE xf6065e43313139d7787a1a85f0b7bf595b14bde1b79d9e652adc2aef89c2962fe77edd9904cd12970fc09095a97cfb8b78a98d91699a0a44de80e1d7b64bb5d91_
            PUSHSLICE x024c0fb7f19c9879471f2eb3c57cc674d52483d8de673c566ccb6c13c17c16f3cd1_
            P256_CHKSIGNU
        ")
        .expect_int_stack(&[-1]);
    }
}

mod secp256k1 {
    use super::*;

    #[test]
    fn test_ec_recover_bad() {
        expect_exception("ECRECOVER", ExceptionCode::StackUnderflow);
        expect_exception("ZERO ECRECOVER", ExceptionCode::StackUnderflow);
        expect_exception("ZERO ONE ECRECOVER", ExceptionCode::StackUnderflow);
        expect_exception("ZERO ONE TWO ECRECOVER", ExceptionCode::StackUnderflow);
        expect_exception("ZERO ONE TWO NULL ECRECOVER", ExceptionCode::TypeCheckError);
    }

    #[test]
    fn test_ec_recover_good() {
        test_case("
            PUSHINT 22331814027392488307105736075480205742348666473969333634173732071459215699411
            PUSHINT 1
            PUSHINT 72188030107017171866617227647805629756021428596569046199967415849707266278927
            PUSHINT 13282575217854591023620411938469150084981210273956175544447397945087449567053
            ECRECOVER
        ")
        .expect_stack(
            Stack::new()
            .push(int!(4))
            .push(int!(parse "21072357408343070128712378251742180313787053800286652796374427952533996402375"))
            .push(int!(parse "90884071343725393946073044816555309859522368395601566415999734440459367428202"))
            .push(boolean!(true))
        );

        test_case(
            "
            PUSHINT 22331814027392488307105736075480205742348666473969333634173732071459215699411
            PUSHINT 4
            PUSHINT 72188030107017171866617227647805629756021428596569046199967415849707266278927
            PUSHINT 13282575217854591023620411938469150084981210273956175544447397945087449567053
            ECRECOVER
        ",
        )
        .expect_int_stack(&[0]);
    }

    #[test]
    fn test_xonly_pubkey_tweak_add_bad() {
        expect_exception(
            "ZERO TRUE SECP256K1_XONLY_PUBKEY_TWEAK_ADD",
            ExceptionCode::RangeCheckError,
        );
        expect_exception(
            "ZERO PUSHNAN SECP256K1_XONLY_PUBKEY_TWEAK_ADD",
            ExceptionCode::RangeCheckError,
        );
        expect_exception("SECP256K1_XONLY_PUBKEY_TWEAK_ADD", ExceptionCode::StackUnderflow);
        expect_exception("ZERO SECP256K1_XONLY_PUBKEY_TWEAK_ADD", ExceptionCode::StackUnderflow);
        expect_exception(
            "ZERO NULL SECP256K1_XONLY_PUBKEY_TWEAK_ADD",
            ExceptionCode::TypeCheckError,
        );
        expect_exception(
            "NULL ZERO SECP256K1_XONLY_PUBKEY_TWEAK_ADD",
            ExceptionCode::TypeCheckError,
        );
    }

    #[test]
    fn test_xonly_pubkey_tweak_add_good() {
        test_case("
            PUSHINT 67440697529755357170833902638082027757174990339341203083254101640286959872673
            PUSHINT 74924293035910479391722402064445116846233519489532129672590201416884966237605 ; [0xA5; 32]
            SECP256K1_XONLY_PUBKEY_TWEAK_ADD
        ")
        .expect_stack(
            Stack::new()
            .push(int!(4))
            .push(int!(parse "64413765334373615256431619272192145611940832188761109400357366900816717656493"))
            .push(int!(parse "38413212773925975206491627475824297433573791488541950698576399555806511550725"))
            .push(boolean!(true))
        );

        // TODO: check PUSHINT 1115792089237316195423570985008687907853269984665640564039457584007913129639935
        test_case("
            PUSHINT 67440697529755357170833902638082027757174990339341203083254101640286959872673
            PUSHPOW2DEC 256
            ; PUSHINT 1115792089237316195423570985008687907853269984665640564039457584007913129639935 ; [0xFF; 32]
            SECP256K1_XONLY_PUBKEY_TWEAK_ADD
        ")
        .expect_int_stack(&[0]);
    }
}

mod ristretto {
    use super::*;

    #[test]
    fn test_push_l() {
        test_case(
            "
            RIST255_PUSHL
            PUSHINT 7237005577332262213973186563042994240857116359379907606001950938285454250989
            EQUAL
        ",
        )
        .expect_int_stack(&[-1]);
    }

    #[test]
    fn test_from_hash_bad() {
        expect_exception("RIST255_FROMHASH", ExceptionCode::StackUnderflow);
        expect_exception("ZERO RIST255_FROMHASH", ExceptionCode::StackUnderflow);
        expect_exception("NULL ZERO RIST255_FROMHASH", ExceptionCode::TypeCheckError);
        expect_exception("ZERO NULL RIST255_FROMHASH", ExceptionCode::TypeCheckError);
        expect_exception("ZERO PUSHNAN RIST255_FROMHASH", ExceptionCode::RangeCheckError);
    }

    #[test]
    fn test_from_hash_good() {
        let hash = "a7903158833c9e8fd2d3bf44223b79ca40683e45e4c66157588713d536e1459209337ba0d3826edea51d9a088eab5110a1f55714680f97e0a96bb21e130eab9d";
        let h1 = &hash[..64];
        let h2 = &hash[64..];
        test_case(format!("
            PUSHINT 0x{h1}
            PUSHINT 0x{h2}
            RIST255_FROMHASH
        "))
        .expect_success()
        .expect_gas(1000000000, 1000000000, 0, 999999323)
        .expect_stack(Stack::new().push(int!(parse "106766070510617938807695755472512202883645982493097127265543572015867144473941")));
    }

    #[test]
    fn test_validate_bad() {
        expect_exception("RIST255_VALIDATE", ExceptionCode::StackUnderflow);
        expect_exception("NULL RIST255_VALIDATE", ExceptionCode::TypeCheckError);
        expect_exception("ONE RIST255_VALIDATE", ExceptionCode::RangeCheckError);

        expect_exception("RIST255_QVALIDATE", ExceptionCode::StackUnderflow);
        expect_exception("NULL RIST255_QVALIDATE", ExceptionCode::TypeCheckError);
    }

    #[test]
    fn test_validate_good() {
        test_case(
            "
            PUSHINT 106766070510617938807695755472512202883645982493097127265543572015867144473941
            RIST255_VALIDATE
        ",
        )
        .expect_int_stack(&[]);

        test_case(
            "
            PUSHINT 106766070510617938807695755472512202883645982493097127265543572015867144473941
            RIST255_QVALIDATE
        ",
        )
        .expect_int_stack(&[-1]);
        test_case("ONE RIST255_QVALIDATE").expect_int_stack(&[0]);
    }

    #[test]
    fn test_add_bad() {
        expect_exception("RIST255_ADD", ExceptionCode::StackUnderflow);
        expect_exception("ZERO RIST255_ADD", ExceptionCode::StackUnderflow);
        expect_exception("NULL NULL RIST255_ADD", ExceptionCode::TypeCheckError);
        expect_exception("NULL ZERO RIST255_ADD", ExceptionCode::TypeCheckError);
        expect_exception("ZERO NULL RIST255_ADD", ExceptionCode::TypeCheckError);
        expect_exception("ONE ONE RIST255_ADD", ExceptionCode::RangeCheckError);

        expect_exception("RIST255_QADD", ExceptionCode::StackUnderflow);
        expect_exception("ZERO RIST255_QADD", ExceptionCode::StackUnderflow);
        expect_exception("NULL NULL RIST255_QADD", ExceptionCode::TypeCheckError);
        expect_exception("NULL ZERO RIST255_QADD", ExceptionCode::TypeCheckError);
        expect_exception("ZERO NULL RIST255_QADD", ExceptionCode::TypeCheckError);
    }

    #[test]
    fn test_add_good() {
        test_case("
            PUSHINT 106766070510617938807695755472512202883645982493097127265543572015867144473941
            PUSHINT 106766070510617938807695755472512202883645982493097127265543572015867144473941
            RIST255_ADD
        ")
        .expect_stack(Stack::new().push(int!(parse "109551166387579915578826152533938171977715400613154730820483154358912285137228")));

        test_case("
            PUSHINT 106766070510617938807695755472512202883645982493097127265543572015867144473941
            PUSHINT 106766070510617938807695755472512202883645982493097127265543572015867144473941
            RIST255_QADD
        ")
        .expect_stack(
            Stack::new()
            .push(int!(parse "109551166387579915578826152533938171977715400613154730820483154358912285137228"))
            .push(boolean!(true))
        );

        test_case("ONE ONE RIST255_QADD").expect_int_stack(&[0]);
    }

    #[test]
    fn test_sub_bad() {
        expect_exception("RIST255_SUB", ExceptionCode::StackUnderflow);
        expect_exception("ZERO RIST255_SUB", ExceptionCode::StackUnderflow);
        expect_exception("NULL NULL RIST255_SUB", ExceptionCode::TypeCheckError);
        expect_exception("NULL ZERO RIST255_SUB", ExceptionCode::TypeCheckError);
        expect_exception("ZERO NULL RIST255_SUB", ExceptionCode::TypeCheckError);
        expect_exception("ONE ONE RIST255_SUB", ExceptionCode::RangeCheckError);

        expect_exception("RIST255_QSUB", ExceptionCode::StackUnderflow);
        expect_exception("ZERO RIST255_QSUB", ExceptionCode::StackUnderflow);
        expect_exception("NULL NULL RIST255_QSUB", ExceptionCode::TypeCheckError);
        expect_exception("NULL ZERO RIST255_QSUB", ExceptionCode::TypeCheckError);
        expect_exception("ZERO NULL RIST255_QSUB", ExceptionCode::TypeCheckError);
    }

    #[test]
    fn test_sub_good() {
        test_case(
            "
            PUSHINT 106766070510617938807695755472512202883645982493097127265543572015867144473941
            PUSHINT 106766070510617938807695755472512202883645982493097127265543572015867144473941
            RIST255_SUB
        ",
        )
        .expect_int_stack(&[0]);

        test_case(
            "
            PUSHINT 106766070510617938807695755472512202883645982493097127265543572015867144473941
            PUSHINT 106766070510617938807695755472512202883645982493097127265543572015867144473941
            RIST255_QSUB
        ",
        )
        .expect_int_stack(&[0, -1]);

        test_case("ONE ONE RIST255_QSUB").expect_int_stack(&[0]);
    }

    #[test]
    fn test_mul_bad() {
        expect_exception("RIST255_MUL", ExceptionCode::StackUnderflow);
        expect_exception("ZERO RIST255_MUL", ExceptionCode::StackUnderflow);
        expect_exception("NULL NULL RIST255_MUL", ExceptionCode::TypeCheckError);
        expect_exception("NULL ZERO RIST255_MUL", ExceptionCode::TypeCheckError);
        expect_exception("ZERO NULL RIST255_MUL", ExceptionCode::TypeCheckError);
        expect_exception("ONE ONE RIST255_MUL", ExceptionCode::RangeCheckError);

        expect_exception("RIST255_QMUL", ExceptionCode::StackUnderflow);
        expect_exception("ZERO RIST255_QMUL", ExceptionCode::StackUnderflow);
        expect_exception("NULL NULL RIST255_QMUL", ExceptionCode::TypeCheckError);
        expect_exception("NULL ZERO RIST255_QMUL", ExceptionCode::TypeCheckError);
        expect_exception("ZERO NULL RIST255_QMUL", ExceptionCode::TypeCheckError);
    }

    #[test]
    fn test_mul_good() {
        test_case("
            PUSHINT 106766070510617938807695755472512202883645982493097127265543572015867144473941
            TWO
            RIST255_MUL
        ")
        .expect_stack(
            Stack::new()
            .push(int!(parse "109551166387579915578826152533938171977715400613154730820483154358912285137228"))
        );

        test_case("
            PUSHINT 106766070510617938807695755472512202883645982493097127265543572015867144473941
            TWO
            RIST255_QMUL
        ")
        .expect_stack(
            Stack::new()
            .push(int!(parse "109551166387579915578826152533938171977715400613154730820483154358912285137228"))
            .push(boolean!(true))
        );

        test_case("ONE ONE RIST255_QMUL").expect_int_stack(&[0]);
    }

    #[test]
    fn test_mulbase_bad() {
        expect_exception("RIST255_MULBASE", ExceptionCode::StackUnderflow);
        expect_exception("NULL RIST255_MULBASE", ExceptionCode::TypeCheckError);

        expect_exception("RIST255_QMULBASE", ExceptionCode::StackUnderflow);
        expect_exception("NULL RIST255_QMULBASE", ExceptionCode::TypeCheckError);
    }

    #[test]
    fn test_mulbase_good() {
        test_case(
            "
            ZERO
            RIST255_MULBASE
        ",
        )
        .expect_stack(Stack::new().push(int!(0)));

        test_case("
            ONE
            RIST255_MULBASE
        ")
        .expect_stack(
            Stack::new()
            .push(int!(parse "102651481954198948695408991041606107487729423467545670950322577403614277217654"))
        );

        test_case("
            RIST255_PUSHL
            INC
            RIST255_MULBASE
        ")
        .expect_stack(
            Stack::new()
            .push(int!(parse "102651481954198948695408991041606107487729423467545670950322577403614277217654"))
        );

        test_case("
            ONE
            RIST255_QMULBASE
        ")
        .expect_stack(
            Stack::new()
            .push(int!(parse "102651481954198948695408991041606107487729423467545670950322577403614277217654"))
            .push(boolean!(true))
        );
    }
}

mod hashext {
    use super::*;
    static CMDS: [&str; 5] = ["SHA256", "SHA512", "BLAKE2B", "KECCAK256", "KECCAK512"];

    #[test]
    fn test_hashext_bad() {
        let simple_code_and_error = [
            ("NULL", ExceptionCode::TypeCheckError),
            ("ONE", ExceptionCode::RangeCheckError),
            ("ONE ONE", ExceptionCode::TypeCheckError),
            ("PUSHSLICE x7F_ ONE", ExceptionCode::CellUnderflow),
        ];
        let append_code_and_error = [
            ("ONE PUSHSLICE x7F_ ONE", ExceptionCode::CellUnderflow),
            ("NEWC PUSHSLICE x7F_ ONE", ExceptionCode::CellUnderflow),
            ("ONE PUSHSLICE x17 PUSHSLICE x7F TWO", ExceptionCode::TypeCheckError),
            (
                "
                NEWC
                PUSHINT 800
                STZEROES
                PUSHSLICE x17
                PUSHSLICE x7F
                TWO
            ",
                ExceptionCode::CellOverflow,
            ),
        ];
        for cmd in &CMDS {
            let code = format!("HASHEXT_{}", cmd);
            expect_exception(&code, ExceptionCode::StackUnderflow);
            for (c, error) in simple_code_and_error {
                let code = format!("{} HASHEXT_{}", c, cmd);
                expect_exception(&code, error);
                let code = format!("{} HASHEXTR_{}", c, cmd);
                expect_exception(&code, error);
                let code = format!("NEWC {} HASHEXTA_{}", c, cmd);
                expect_exception(&code, error);
                let code = format!("NEWC {} HASHEXTAR_{}", c, cmd);
                expect_exception(&code, error);
            }
            for (c, error) in append_code_and_error {
                let code = format!("{} HASHEXTA_{}", c, cmd);
                expect_exception(&code, error);
                let code = format!("{} HASHEXTAR_{}", c, cmd);
                expect_exception(&code, error);
            }
        }
    }

    #[test]
    fn test_hashext_good() {
        let hash_256 = |hash: &str, hash_rev: &str| {
            let hash_int = int!(parse_hex hash);
            let hash_bld =
                StackItem::builder(SliceData::from_string(hash).unwrap().into_builder().unwrap());
            let hash_rev_int = int!(parse_hex hash_rev);
            let hash_rev_bld = StackItem::builder(
                SliceData::from_string(hash_rev).unwrap().into_builder().unwrap(),
            );
            (hash_int, hash_bld, hash_rev_int, hash_rev_bld)
        };

        let hash_512 = |hash: &str, hash_rev: &str| {
            let hash_l = &hash[..64];
            let hash_h = &hash[64..];
            let hash_tup = StackItem::tuple(vec![int!(parse_hex hash_l), int!(parse_hex hash_h)]);
            let hash_bld =
                StackItem::builder(SliceData::from_string(hash).unwrap().into_builder().unwrap());
            let hash_l = &hash_rev[..64];
            let hash_h = &hash_rev[64..];
            let hash_rev_tup =
                StackItem::tuple(vec![int!(parse_hex hash_l), int!(parse_hex hash_h)]);
            let hash_rev_bld = StackItem::builder(
                SliceData::from_string(hash_rev).unwrap().into_builder().unwrap(),
            );
            (hash_tup, hash_bld, hash_rev_tup, hash_rev_bld)
        };

        let mut hashes = Vec::new();
        hashes.push(hash_256(
            "21d8c3bf65059083399bf787edc42818790a174d27709efef2d80763462d70f7",
            "4e6c2a7fa9d299fcfd65e18f368606b49a29e228d29a01f67f4191a3618d9802",
        ));
        hashes.push(hash_512(
            "D07DDB4D029752C132401BCAA10CC5ED8782D09F98A3B45AFAE66EFD7EE2BB7438CFA574EFC1963A42A71DBD6DEF15C215936E649D41CA7933A7F17F23771F3C",
            "1C6BBD616302B373B21B42A3C7F0512001B6642D358CD663BD4774E1D2132F9EB4EAA8EDF74EF42126B6FEEE6CFF1C2B259BE3ED7CDD3AF8022769BAE36821D7"
        ));
        hashes.push(hash_512(
            "97A9DFC2B3750373811CD74DA1FAC7F5402EA9EF8D6345A945756C8A0F9420A42D215455FE52C2C5551612D71EA02247DBE344BB10CB88DCD543B25AC335B17C",
            "D61660F0FC72BAFCE7989E47D22FC2CD1B7EA0D4CDAF735EE3675191C4F2B57B64ACBB505960B5D0138426D5DE9E2E8F56C702FC06D08BE4515EC53993B874AA"
        ));
        hashes.push(hash_256(
            "0428FDD7194CD73E5E54A1F8A14D6906F74F47E89FE591A91BF5089959F11705",
            "3CA02B6DC8040DB12894BA5AE53BDDE9247E4675C0B4832F6F05A8CF64B7297C",
        ));
        hashes.push(hash_512(
            "75F74D61A1F96E6090950B3D59A4B05D49BD36BEDB18597B1DAA90E51FDF7BB6F97319BD36FA706DBE48B76D3494E30CDC7EC8EB1DF0C1C6A990A8208D8A1EDD",
            "78548D01E4B307F19237523F29A99F39208D9C05BF701F418E74E678872D84E636ECF825C39D0B2F2A8A53E671FA96892EECBC0A83EA8E398BD4A37ED9DE592C"
        ));

        for ((hash, hash_bld, hash_rev, hash_rev_bld), cmd) in hashes.into_iter().zip(CMDS.iter()) {
            let code = format!("PUSHSLICE x257E ONE HASHEXT_{}", cmd);
            test_case(&code).expect_item(hash.clone());

            let code = format!("PUSHSLICE x257F_ PUSHSLICE x4_ TWO HASHEXT_{}", cmd);
            test_case(&code).expect_item(hash.clone());

            let code = format!("PUSHSLICE x257E ONE HASHEXTR_{}", cmd);
            test_case(&code).expect_item(hash.clone());

            let code = format!("NEWC PUSHSLICE x257E ONE HASHEXTA_{}", cmd);
            test_case(&code).expect_item(hash_bld.clone());

            let code = format!("PUSHSLICE x257F_ PUSHSLICE x4_ TWO HASHEXTR_{}", cmd);
            test_case(&code).expect_item(hash_rev.clone());

            let code = format!("NEWC PUSHSLICE x257F_ PUSHSLICE x4_ TWO HASHEXTAR_{}", cmd);
            test_case(&code).expect_item(hash_rev_bld.clone());
        }
    }
}

#[ignore]
#[test]
fn run_real_transaction() {
    let prefix = "../target/cmp/validator/0,a000000000000000,53188601/";
    test_case_with_real_data(
        &format!("{prefix}/mc_state_proof.boc"),
        &format!("{prefix}/account_old.boc"),
        &format!("{prefix}/message.boc"),
    )
    .expect_success();
}
