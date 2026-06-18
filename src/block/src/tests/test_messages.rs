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
use super::*;
use crate::{generate_test_message, generate_test_stateinit, write_boc, StateInitTestOptions};

#[cfg(test)]
pub fn write_read_and_assert_message(msg: Message) {
    let cell = msg.serialize().unwrap();
    let mut slice = SliceData::load_cell_ref(&cell).unwrap();
    println!("slice: {}", slice);
    let s2 = Message::construct_from(&mut slice).unwrap();
    let cell2 = s2.serialize().unwrap();
    pretty_assertions::assert_eq!(msg, s2);
    if cell != cell2 {
        panic!("write_read_and_assert: cells are not equal\nleft: {cell:#.5}\nright: {cell2:#.5}")
    }
    let bytes = write_boc(&cell).unwrap();
    let header = Message::read_header_fast(&bytes).unwrap();
    assert_eq!(&header, msg.header());
}

#[test]
fn test_serialize_many_times() {
    let stinit = generate_test_stateinit(StateInitTestOptions::with_default_setup(true));
    let mut msg = Message::with_int_header(InternalMessageHeader::default());
    let body = SliceData::new(vec![0x3F, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xF4]);
    msg.init = Some(stinit);
    msg.set_body(body);

    let b1 = msg.write_to_new_cell().unwrap();
    let b2 = msg.write_to_new_cell().unwrap();
    let b3 = msg.write_to_new_cell().unwrap();
    assert_eq!(b1, b2);
    assert_eq!(b1, b3);
}

#[test]
fn test_serialize_simple_messages() {
    let msg = Message::with_int_header(InternalMessageHeader::default());
    write_read_and_assert_message(msg);
    let msg = Message::with_ext_in_header(ExternalInboundMessageHeader::default());
    write_read_and_assert_message(msg);
    let msg = Message::with_ext_out_header(ExtOutMessageHeader::default());
    write_read_and_assert_message(msg);
}

#[test]
fn test_serialize_msg_with_state_init() {
    let stinit = generate_test_stateinit(StateInitTestOptions::with_default_setup(false));
    let mut msg = Message::with_int_header(InternalMessageHeader::default());
    msg.init = Some(stinit);
    write_read_and_assert_message(msg);
}

#[test]
fn test_save_external_serialization_order() {
    let mut msg = Message::with_int_header(InternalMessageHeader::default());
    let body = SliceData::new(vec![0x55; 64]);
    msg.set_body(body);
    msg.set_serialization_params(Some(true), Some(false));

    let b = msg.serialize().unwrap();
    let m1 = Message::construct_from_cell(b).unwrap();
    println!("{:?}", m1.serialization_params());
    assert_eq!(m1.serialization_params(), (Some(true), Some(false)));
    assert_eq!(msg, m1);
}

#[test]
fn test_serialize_msg_with_state_init_code_and_small_body() {
    let mut stinit = StateInit::default();
    let code = SliceData::new(vec![0x3F, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xF4]);
    stinit.set_code(code.into_cell().unwrap());
    let body = SliceData::new(vec![0x55; 64]);
    let mut msg = Message::with_int_header(InternalMessageHeader::default());
    msg.init = Some(stinit);
    msg.set_body(body);
    write_read_and_assert_message(msg);
}

#[test]
fn test_serialize_msg_with_state_init_and_body() {
    let stinit = generate_test_stateinit(StateInitTestOptions::with_default_setup(true));
    let body = SliceData::new(vec![0x3F, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xF4]);
    let mut msg = Message::with_int_header(InternalMessageHeader::default());
    msg.init = Some(stinit);
    msg.set_body(body);
    write_read_and_assert_message(msg);
}

#[test]
fn test_serialize_msg_with_state_init_and_big_body() {
    let msg = generate_test_message(false, StateInitTestOptions::with_default_setup(true));
    write_read_and_assert_message(msg);
}

#[test]
fn test_serialize_msg_with_state_init_with_refs_and_big_body_with_refs() {
    let msg = generate_test_message(true, StateInitTestOptions::with_default_setup(true));
    write_read_and_assert_message(msg);
}

#[test]
fn test_check_message_output() {
    let mut msg = Message::with_int_header(InternalMessageHeader::with_addresses_and_bounce(
        MsgAddressInt::with_variant(None, -1, SliceData::new(vec![12, 13, 17])).unwrap(),
        MsgAddressInt::with_standart(
            Some(AnycastInfo::with_rewrite_pfx(SliceData::new(vec![0xC4])).unwrap()),
            5,
            [55; 32].into(),
        )
        .unwrap(),
        CurrencyCollection::with_coins(79),
        false,
    ));
    let mut stinit = StateInit::default();
    stinit.set_fixed_prefix_length(23.try_into().unwrap());
    stinit.set_special(TickTock::with_values(false, true));
    let code = SliceData::new(vec![0x3F, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xF4]);
    stinit.set_code(code.into_cell().unwrap());
    let library = SliceData::new(vec![0x3F, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xF4]);
    stinit.set_library_code(library.into_cell().unwrap(), false).unwrap();
    msg.init = Some(stinit);
    msg.set_body(SliceData::new(vec![0x55, 0x55, 0x80]));
    pretty_assertions::assert_eq!(
        format!("{}", msg),
        "Message {header: Internal {src: -1:0c0d11_, \
            dst: c4_:5:3737373737373737373737373737373737373737373737373737373737373737}, \
            init to ref: None, StateInit { fixed_prefix_length: Some(Number5(23)), \
            special: Some(TickTock { tick: false, tock: true }), \
            code: Some(7a0b957a15e93cca3ce96ccb4aecf275a3718a263c8aeca2ab14fe6e1e62172c), \
            data: None, \
            library: StateInitLib(HashmapE { bit_len: 256, \
            data: Some(c39760fbba54774b6c7fa76bfd46d6fb89d1fe0b19570bef3c4d08decc8b4566) }) }, \
            body to ref: None, 5555\
        }"
    );
    write_read_and_assert_message(msg.clone());

    let mut body = SliceData::new_empty();
    body.append_reference(SliceData::new(vec![0x55, 0x55, 0x80])).unwrap();
    msg.set_body(body);
    pretty_assertions::assert_eq!(
        format!("{}", msg),
        "Message {header: Internal {src: -1:0c0d11_, \
            dst: c4_:5:3737373737373737373737373737373737373737373737373737373737373737}, \
            init to ref: None, StateInit { fixed_prefix_length: Some(Number5(23)), \
            special: Some(TickTock { tick: false, tock: true }), \
            code: Some(7a0b957a15e93cca3ce96ccb4aecf275a3718a263c8aeca2ab14fe6e1e62172c), \
            data: None, \
            library: StateInitLib(HashmapE { bit_len: 256, \
            data: Some(c39760fbba54774b6c7fa76bfd46d6fb89d1fe0b19570bef3c4d08decc8b4566) }) }, \
            body to ref: None, \
        }"
    );
    write_read_and_assert_message(msg);
}

#[test]
fn test_check_json_address() {
    let address = "Ef9VVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVbxn";
    assert_eq!(MsgAddressInt::standard(-1, [0x55; 32]), address.parse().unwrap());

    assert_eq!(
        MsgAddressInt::from_str("EQAmuU93tDFfvXr3So6WsikFwdgSDSg-s1PckeK9a4ZHjlyl").unwrap(), // base64-url
        MsgAddressInt::from_str(
            "0:26b94f77b4315fbd7af74a8e96b22905c1d8120d283eb353dc91e2bd6b86478e"
        )
        .unwrap()
    );

    let address = "EQAmuU93tDFfvXr3So6WsikFwdgSDSg+s1PckeK9a4ZHjly(";
    address.parse::<MsgAddressInt>().expect_err("base64 format error");

    let address = "EQAmuU93tDFfvXr3So6WsikFwdgSDSg+s1PckeK9a4ZHjlyl";
    address.parse::<MsgAddressInt>().expect_err("base64_url_safe format error");

    let address = "EQAmuU93tDFfvXr3So6WsikFwdgSDSg-s1PckeK9a4ZHjg";
    address.parse::<MsgAddressInt>().expect_err("length error");

    let address = "EQAmuU93tDFfvXr3So6WsikFwdgSDSg-s1PckeK9a4ZHjlyl____";
    address.parse::<MsgAddressInt>().expect_err("length error");

    let address = "EQAmuU93tDFfvXr3So6WsikFwdgSDSg-s1PckeK9a4ZHjv__";
    address.parse::<MsgAddressInt>().expect_err("CRC error");

    let addresses = [
        " ",
        "11:",
        "q:22",
        ":-33",
        ":44 ",
        ":0:55",
        " :66",
        " :77 ",
        "2147483648:33",
        "-2147483649:44",
        ":0:66",
        "66 ",
        " 66",
        " 66 ",
        "12345678:0:66",
        "0:555555555555555555555555555555555555555555555555555555555555555",
        "-1:555555555555555555555555555555555555555555555555555555555555555555",
    ];

    addresses.iter().for_each(|addr| {
        let err = MsgAddressInt::from_str(addr).err();
        println!("{:?}", err);
        assert!(err.is_some());
    });

    let anycast = AnycastInfo::with_rewrite_pfx(SliceData::new(vec![0x77, 0x80])).unwrap();
    let addresses_int = [
        ("255:55_", MsgAddressInt::with_variant(None, 255, SliceData::new(vec![0x55])).unwrap()),
        (
            "77:-129:55_",
            MsgAddressInt::with_variant(Some(anycast.clone()), -129, SliceData::new(vec![0x55]))
                .unwrap(),
        ),
        (
            "0:5555555555555555555555555555555555555555555555555555555555555555",
            MsgAddressInt::with_standart(None, 0, AccountId::from([0x55; 32])).unwrap(),
        ),
        (
            "1:5555555555555555555555555555555555555555555555555555555555555555",
            MsgAddressInt::with_standart(None, 1, AccountId::from([0x55; 32])).unwrap(),
        ),
        (
            "77:1:5555555555555555555555555555555555555555555555555555555555555555",
            MsgAddressInt::with_standart(Some(anycast), 1, AccountId::from([0x55; 32])).unwrap(),
        ),
        (
            "128:5555555555555555555555555555555555555555555555555555555555555555",
            MsgAddressInt::with_variant(None, 128, AccountId::from([0x55; 32])).unwrap(),
        ),
        (
            "1:55555555555555555555555555555555555555555555555555555555555555558_",
            MsgAddressInt::with_variant(None, 1, AccountId::from([0x55; 32])).unwrap(),
        ),
        (
            "0:55555555555555555555555555555555555555555555555555555555555555558_",
            MsgAddressInt::with_variant(None, 0, AccountId::from([0x55; 32])).unwrap(),
        ),
        (
            "1111:8888",
            MsgAddressInt::with_variant(None, 1111, SliceData::new(vec![0x88, 0x88, 0x80]))
                .unwrap(),
        ),
        (
            "1111:777",
            MsgAddressInt::with_variant(None, 1111, SliceData::new(vec![0x77, 0x78])).unwrap(),
        ),
        (
            "1111:abc_",
            MsgAddressInt::with_variant(None, 1111, SliceData::new(vec![0xAB, 0xC0])).unwrap(),
        ),
    ];
    addresses_int.iter().for_each(|(addr, check)| {
        let real = MsgAddressInt::from_str(addr).unwrap();
        println!("{}", real);
        assert_eq!(&real, check);
        assert_eq!(&format!("{}", real), addr);
    });

    let addresses_ext = [
        ("", MsgAddressExt::AddrNone),
        (":55_", MsgAddressExt::with_extern(SliceData::new(vec![0x55])).unwrap()),
        (
            ":5555555555555555555555555555555555555555555555555555555555555555",
            MsgAddressExt::with_extern(AccountId::from([0x55; 32])).unwrap(),
        ),
    ];
    addresses_ext.iter().for_each(|(addr, check)| {
        let real = MsgAddressExt::from_str(addr).unwrap();
        println!("{}", real);
        assert_eq!(&real, check);
        assert_eq!(&format!("{}", real), addr);
    });
}

#[test]
fn test_message_addr_external_err_1() {
    let err = MsgAddressExt::with_extern(SliceData::from_raw(vec![1; 65], 513)).err();
    assert!(err.is_some());
}

#[test]
fn test_message_addr_external_err_2() {
    let err = MsgAddressExt::from_str(
        ":23232323232323232323232323232323232323232323232323232323232323\
         232323232323232323232323232323232323232323232323232323232323232\
         32333",
    )
    .err();
    assert!(err.is_some());
}

#[test]
fn test_msg_address_int_or_none() {
    let addr1 = MsgAddressIntOrNone::default();
    let addr2str = "-1:5555555555555555555555555555555555555555555555555555555555555555";
    let addr2 = MsgAddressIntOrNone::Some(MsgAddressInt::from_str(addr2str).unwrap());
    let addr3 = MsgAddressExt::with_extern(SliceData::new(vec![0x55])).unwrap();
    let mut b = BuilderData::new();
    addr1.write_to(&mut b).unwrap();
    addr2.write_to(&mut b).unwrap();
    addr3.write_to(&mut b).unwrap();
    let mut s = SliceData::load_builder(b).unwrap();
    println!("{:x}", s);
    let mut addr1_ = MsgAddressIntOrNone::default();
    addr1_.read_from(&mut s).unwrap();
    assert_eq!(addr1, addr1_);
    let mut addr2_ = MsgAddressIntOrNone::default();
    addr2_.read_from(&mut s).unwrap();
    assert_eq!(addr2, addr2_);
    let mut addr3_ = MsgAddressIntOrNone::default();
    let err = addr3_.read_from(&mut s).err();
    assert!(err.is_some());
}

#[test]
fn test_msg_address_int_invalid() {
    let addr1 = MsgAddressIntOrNone::default();
    let b = addr1.write_to_new_cell().unwrap();
    let mut s = SliceData::load_builder(b).unwrap();
    println!("{:x}", s);
    MsgAddressInt::construct_from(&mut s)
        .expect_err("MsgAddressInt should not be deserialized from None");
}

#[test]
fn test_base64_url_encode() {
    let address = "EQCxE6mUtQJKFnGfaROTKOt1lZbDiiX1kCixRv7Nw2Id_sDs";
    let addr: MsgAddressInt = address.parse().unwrap();
    assert_eq!(
        addr.to_string_custom(0).unwrap(),
        "0:b113a994b5024a16719f69139328eb759596c38a25f59028b146fecdc3621dfe"
    );
    assert_eq!(
        addr.to_string_custom(ADDR_FORMAT_URL_SAFE).unwrap(),
        "UQCxE6mUtQJKFnGfaROTKOt1lZbDiiX1kCixRv7Nw2Id_p0p"
    );
    assert_eq!(
        addr.to_string_custom(ADDR_FORMAT_URL_SAFE | ADDR_FORMAT_BOUNCE).unwrap(),
        "EQCxE6mUtQJKFnGfaROTKOt1lZbDiiX1kCixRv7Nw2Id_sDs"
    );
    assert_eq!(
        addr.to_string_custom(ADDR_FORMAT_URL_SAFE | ADDR_FORMAT_TESTNET).unwrap(),
        "0QCxE6mUtQJKFnGfaROTKOt1lZbDiiX1kCixRv7Nw2Id_iaj"
    );
    assert_eq!(
        addr.to_string_custom(ADDR_FORMAT_URL_SAFE | ADDR_FORMAT_BOUNCE | ADDR_FORMAT_TESTNET)
            .unwrap(),
        "kQCxE6mUtQJKFnGfaROTKOt1lZbDiiX1kCixRv7Nw2Id_ntm"
    );
}

#[test]
fn test_external_inbound_message_normalize() {
    let h = ExternalInboundMessageHeader {
        src: MsgAddressExt::with_extern(SliceData::new(vec![77, 0x80])).unwrap(),
        dst: MsgAddressInt::standard(-1, [0x55; 32]),
        import_fee: Coins::new(12345678),
    };
    let body = SliceData::new(vec![0xde, 0xad, 0xbe, 0xef, 0x80]);
    let mut msg = Message::with_ext_in_header_and_body(h, body);
    println!("msg: {}", msg.write_to_base64().unwrap());

    let hash = msg.serialize().unwrap().repr_hash().clone();
    let hash = crate::base64_encode(&hash);
    assert_eq!(hash, "ZaaNsPLNFFGwWHZJCBM/LHiSyjbbPDBtpjS/ccx96vY=");

    msg.normalize_external_inbound().unwrap();
    let cell = msg.serialize().unwrap();
    println!("{cell:#.10}");
    let hash = cell.repr_hash();
    let hash = crate::base64_encode(&hash);
    assert_eq!(hash, "D6MlSVvEDTsja7zG0wNHiRzDj3AxurTkSFHJcKItEes=");
}
