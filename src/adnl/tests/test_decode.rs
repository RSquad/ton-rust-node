/*
 * Copyright (C) 2019-2023 EverX. All Rights Reserved.
 * Modifications Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This file has been modified from its original version.
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use adnl::common::Version;
use ton_api::{
    deserialize_boxed,
    ton::{
        rldp::{message::Query as RldpQuery, Message as RldpMessage},
        rpc::{overlay::Query as OverlayQuery, ton_node::DownloadBlock},
    },
    IntoBoxed, TLObject,
};
use ton_block::UInt256;

fn print_tl_object(answer: TLObject) {
    if answer.is::<RldpMessage>() {
        let answer = answer.downcast::<RldpMessage>().unwrap();
        println!("{:?}", answer);
        let data = &answer.data();
        println!("{:?}", hex::encode(data));
    } else if answer.is::<OverlayQuery>() {
        println!("{:?}", answer.downcast::<OverlayQuery>().unwrap());
    } else if answer.is::<DownloadBlock>() {
        println!("{:?}", answer.downcast::<DownloadBlock>().unwrap());
    } else {
        println!("{:?}", answer);
    }
}

#[test]
fn test_decode() {
    let query_msg_str =
        "4384fdccdc7c6d60991db081780e7e12627d8c315dc171db982452e91f1f30d738cef966c37972e2ffffffff\
         000000000000008097920a00bc8f430b9ae5817be1fa1974918df336dbc5678088e6ebbb9f6cb027ad0ea24b\
         f88dafbafb920d0feb50d4ca8241d920c48eaf63cb20924961ffd6f39966cb4d";

    let query_msg = hex::decode(query_msg_str).unwrap();
    let query1 = deserialize_boxed(query_msg).unwrap();
    print_tl_object(query1);
    // let query2 = deserializer.read_boxed().unwrap();
    // print_tl_object(query2);

    let now = Version::get();
    let data = hex::decode(
        "4384fdccdc7c6d60991db081780e7e12627d8c315dc171db982452e91f1f30d738cef966c37972e2ffffffff\
         000000000000008097920a00bc8f430b9ae5817be1fa1974918df336dbc5678088e6ebbb9f6cb027ad0ea24b\
         f88dafbafb920d0feb50d4ca8241d920c48eaf63cb20924961ffd6f39966cb4d",
    )
    .unwrap();

    let q = RldpQuery {
        query_id: UInt256::with_array([12; 32]),
        max_answer_size: 4194304,
        timeout: now + 3600,
        data: data.into(),
    }
    .into_boxed();
    println!("{:?}", q);
}
