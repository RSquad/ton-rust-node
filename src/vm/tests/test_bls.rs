/*
 * Copyright (C) 2019-2024 TON Labs. All Rights Reserved.
 * Modifications Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This file has been modified from its original version.
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use ton_block::{aggregate_pure_bls_signatures, gen_bls_key_pair, sign, ExceptionCode};
use ton_vm::{
    boolean, int,
    stack::{integer::IntegerData, Stack, StackItem},
};

mod common;
use common::*;

#[test]
fn test_bls_verify() {
    let (pub_key, secret_key) = gen_bls_key_pair().unwrap();
    let message = "Hello, BLS verify!".as_bytes();
    let signature = sign(&secret_key, message).unwrap();

    let code = format!(
        "
        PUSHSLICE x{pk} ; public key
        PUSHSLICE x{msg} ; message
        PUSHSLICE x{sig} ; signature
        BLS_VERIFY",
        pk = hex::encode(pub_key.as_slice()),
        msg = hex::encode(message),
        sig = hex::encode(signature.as_slice())
    );

    test_case(code).expect_success().expect_stack(Stack::new().push(boolean!(true)));
}

#[test]
fn test_bls_verify_bad_data() {
    let code = format!("
        PUSHSLICE xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff ; public key
        PUSHSLICE xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff ; message
        PUSHSLICE xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff ; signature
        BLS_VERIFY",
    );

    test_case(code).expect_success().expect_stack(Stack::new().push(boolean!(false)));
}

#[test]
fn test_bls_verify_bad_data2() {
    let code = format!(
        "
        PUSHSLICE x123 ; public key
        PUSHSLICE x456 ; message
        PUSHSLICE x789 ; signature
        BLS_VERIFY",
    );

    test_case(code).expect_failure(ExceptionCode::CellUnderflow);
}

#[test]
fn test_bls_aggregate_simple() {
    let message = "Hello, BLS aggregate!".as_bytes();
    let mut signatures = vec![];
    for _ in 0..3 {
        let (_pub_key, secret_key) = gen_bls_key_pair().unwrap();
        signatures.push(sign(&secret_key, message).unwrap());
    }
    let aggr_signature =
        aggregate_pure_bls_signatures(&[&signatures[0], &signatures[1], &signatures[2]]).unwrap();
    let mut aggr_signature = aggr_signature.as_slice().to_vec();
    aggr_signature.push(0x80);

    let code = format!(
        "
        PUSHSLICE x{sig1}
        PUSHSLICE x{sig2}
        PUSHSLICE x{sig3}
        PUSHINT 3
        BLS_AGGREGATE",
        sig1 = hex::encode(signatures[0].as_slice()),
        sig2 = hex::encode(signatures[1].as_slice()),
        sig3 = hex::encode(signatures[2].as_slice())
    );

    test_case(code)
        .expect_success()
        .expect_stack(Stack::new().push(create::slice(&aggr_signature)));
}

#[test]
fn test_bls_fast_aggregate_verify() {
    let message = "Hello, BLS fast aggregate and verify!".as_bytes();
    let mut pub_keys = vec![];
    let mut signatures = vec![];
    for _ in 0..3 {
        let (pub_key, secret_key) = gen_bls_key_pair().unwrap();
        signatures.push(sign(&secret_key, message).unwrap());
        pub_keys.push(pub_key);
    }
    let aggr_signature =
        aggregate_pure_bls_signatures(&[&signatures[0], &signatures[1], &signatures[2]]).unwrap();

    let code = format!(
        "
        PUSHSLICE x{pk1}
        PUSHSLICE x{pk2}
        PUSHSLICE x{pk3}
        PUSHINT 3
        PUSHSLICE x{msg}
        PUSHSLICE x{sig}
        BLS_FASTAGGREGATEVERIFY",
        pk1 = hex::encode(pub_keys[0].as_slice()),
        pk2 = hex::encode(pub_keys[1].as_slice()),
        pk3 = hex::encode(pub_keys[2].as_slice()),
        msg = hex::encode(message),
        sig = hex::encode(aggr_signature.as_slice())
    );

    test_case(code).expect_success().expect_stack(Stack::new().push(boolean!(true)));
}

#[test]
fn test_bls_fast_aggregate_verify_bad_data() {
    let code = format!("
        PUSHSLICE x8b1eac18b6e7a38f2b2763c9a03c3b6cff4110f18c4d363eec455463bd5c8671fb81204c4732406d72468a1474df6133147a2240f4073a472ef419f23011ee4d6cf02fceb844398e33e2e331635dace3b26464a6851e10f6895923c568582fbd
        PUSHSLICE x94ec60eb8d2b657dead5e1232b8f9cc0162467b08f02e252e97622297787a74b6496607036089837fe5b52244bbbb6d00d3d7cc43812688451229d9e96f704401db053956c588203ba7638e8882746c16e701557f34b0c08bbe097483aec161e
        PUSHSLICE x8cdbeadb3ee574a4f796f10d656885f143f454cc6a2d42cf8cabcd592d577c5108e4258a7b14f0aafe6c86927b3e70030432a2e5aafa97ee1587bbdd8b69af044734defcf3c391515ab26616e15f5825b4b022a7df7b44f65a8792c54762e579
        PUSHINT 3
        PUSHSLICE x48656c6c6f2c20424c5320666173742061676772656761746520616e642076657269667921
        PUSHSLICE x8420b1944c64f74dd67dc9f5ab210bab928e2edd4ce7e40c6ec3f5422c99322a5a8f3a8527eb31366c9a74752d1dce340d5a98fbc7a04738c956e74e7ba77b278cbc52afc63460c127998aae5aa1c3c49e8c48c30cc92451a0a275a47f219602
        BLS_FASTAGGREGATEVERIFY"
    );

    test_case(code).expect_success().expect_stack(Stack::new().push(boolean!(false)));
}

#[test]
fn test_bls_fast_aggregate_verify_bad_data2() {
    let code = format!("
        PUSHINT 0
        PUSHSLICE x48656c6c6f2c20424c5320666173742061676772656761746520616e642076657269667921
        PUSHSLICE x8420b1944c64f74dd67dc9f5ab210bab928e2edd4ce7e40c6ec3f5422c99322a5a8f3a8527eb31366c9a74752d1dce340d5a98fbc7a04738c956e74e7ba77b278cbc52afc63460c127998aae5aa1c3c49e8c48c30cc92451a0a275a47f219602
        BLS_FASTAGGREGATEVERIFY"
    );

    test_case(code).expect_success().expect_stack(Stack::new().push(boolean!(false)));
}

#[test]
fn test_bls_aggregate_verify() {
    let mut messages = vec![];
    let mut pub_keys = vec![];
    let mut signatures = vec![];
    for i in 0..3 {
        let (pub_key, secret_key) = gen_bls_key_pair().unwrap();
        let message = format!("Hello, BLS fast aggregate and verify {i}!").as_bytes().to_vec();
        signatures.push(sign(&secret_key, &message).unwrap());
        messages.push(message);
        pub_keys.push(pub_key);
    }
    let aggr_signature =
        aggregate_pure_bls_signatures(&[&signatures[0], &signatures[1], &signatures[2]]).unwrap();

    let code = format!(
        "
        PUSHSLICE x{pk1}
        PUSHSLICE x{msg1}
        PUSHSLICE x{pk2}
        PUSHSLICE x{msg2}
        PUSHSLICE x{pk3}
        PUSHSLICE x{msg3}
        PUSHINT 3
        PUSHSLICE x{sig}
        BLS_AGGREGATEVERIFY",
        pk1 = hex::encode(pub_keys[0]),
        msg1 = hex::encode(&messages[0]),
        pk2 = hex::encode(pub_keys[1]),
        msg2 = hex::encode(&messages[1]),
        pk3 = hex::encode(pub_keys[2]),
        msg3 = hex::encode(&messages[2]),
        sig = hex::encode(aggr_signature)
    );

    test_case(code).expect_success().expect_int_stack(&[-1]);
}

#[test]
fn test_bls_aggregate_verify_bad_data() {
    let code = format!("
        PUSHINT 0
        PUSHSLICE x8420b1944c64f74dd67dc9f5ab210bab928e2edd4ce7e40c6ec3f5422c99322a5a8f3a8527eb31366c9a74752d1dce340d5a98fbc7a04738c956e74e7ba77b278cbc52afc63460c127998aae5aa1c3c49e8c48c30cc92451a0a275a47f219602
        BLS_AGGREGATEVERIFY"
    );

    test_case(code).expect_success().expect_stack(Stack::new().push(boolean!(false)));
}

#[test]
fn test_bls_g1_bad_data() {
    for instruction in ["BLS_G1_ADD", "BLS_G1_SUB", "BLS_G1_NEG"] {
        test_case(format!("
            PUSHSLICE xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff
            PUSHSLICE xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff
            {instruction}",
        ))
        .expect_failure(ExceptionCode::UnknownError);
    }
}

#[test]
fn test_bls_g1_bad_data2() {
    test_case("
        PUSHSLICE xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff
        PUSHINT 229
        BLS_G1_MUL",
    )
    .expect_failure(ExceptionCode::UnknownError);
}

#[test]
fn test_bls_g1_bad_data3() {
    test_case(
       "PUSHSLICE x8daf02bb3b3dca16883b251cb1a2eda65d10a29de23d06908a0e3c383b16a525242725a24900c973f1990d4a50de980500c0ede436577136c4c4e810e8308e1894b17a3ec112084cb3b6768e2d52b09da007f2071ec650776e0e0d27e2627e94
        PUSHINT 2
        PUSHSLICE xb24ba624dba737b7476faea0ffc8f0075847cd91de8e816e5e70339e698d197622c1434e2e8fd4658c2891303ccf349c07ec691ecd5243a88498202c48c14d2f7a062b0b2a4459eee9a3914c7af90650c22b9d54a7d276b7c71700faebdafd32
        PUSHINT 5
        PUSHSLICE x91b74a177d2a300d08236464fdeb86ffc68de44c75fd5c0345a5059a73861cc4630335ef846610a5abe05c88c44b6afc003570e735cec70b36a08f1cf78728fc3da4c84f21d578297e55686b0bd9beaf42e1388be3453c846bad14731e30563c
        PUSHINT 13537812947843
        PUSHINT 3
        BLS_G1_MULTIEXP
         ",
    )
    .expect_failure(ExceptionCode::UnknownError);
}

#[test]
fn test_bls_g1_1() {
    test_case(
        "PUSHSLICE x7abd13983c76661a98659da83066c71bd6581baf20c82c825b007bf8057a258dc53f7a6d44fb6fdecb63d9586e845d92
         BLS_MAP_TO_G1
         PUSHSLICE x7a6990b38d5a7bfc47b38c5adeec60680637e8a5030dddd796e7befbec3585c54c378472daadd7756ce7a52adbea507c
         BLS_MAP_TO_G1
         BLS_G1_ADD
         PUSHSLICE x7a6990b38d5a7bfc47b38c5adeec60680637e8a5030dddd796e7befbec3585c54c378472daadd7756ce7a52adbea507c
         BLS_MAP_TO_G1
         BLS_G1_SUB
        ",
    )
    .expect_success()
    .expect_stack(Stack::new()
        .push(create::slice(hex::decode(
            "b1902667aa48b8acfe31802ace1774abad7d0328be2224f5d239b9f8b82172d5077dd56f779db47666d94405bf740d6c80"
        ).unwrap()))
    );
}

#[test]
fn test_bls_g1_2() {
    test_case(
        "PUSHSLICE x7abd13983c76661a98659da83066c71bd6581baf20c82c825b007bf8057a258dc53f7a6d44fb6fdecb63d9586e845d92
         BLS_MAP_TO_G1
         BLS_G1_NEG
         PUSHSLICE x7abd13983c76661a98659da83066c71bd6581baf20c82c825b007bf8057a258dc53f7a6d44fb6fdecb63d9586e845d92
         BLS_MAP_TO_G1
         BLS_G1_ADD
         BLS_G1_ISZERO
        ",
    )
    .expect_success()
    .expect_stack(Stack::new().push(boolean!(true)));
}

#[test]
fn test_bls_g1_3() {
    test_case(
        "PUSHSLICE x7abd13983c76661a98659da83066c71bd6581baf20c82c825b007bf8057a258dc53f7a6d44fb6fdecb63d9586e845d92
         BLS_MAP_TO_G1
         BLS_G1_NEG

         PUSHSLICE x7abd13983c76661a98659da83066c71bd6581baf20c82c825b007bf8057a258dc53f7a6d44fb6fdecb63d9586e845d92
         BLS_MAP_TO_G1
         PUSHINT -1
         BLS_G1_MUL

         BLS_G1_SUB
         BLS_G1_ISZERO
        ",
    )
    .expect_success()
    .expect_stack(Stack::new().push(boolean!(true)));
}

#[test]
fn test_bls_g1_4() {
    test_case(
        "PUSHSLICE x7abd13983c76661a98659da83066c71bd6581baf20c82c825b007bf8057a258dc53f7a6d44fb6fdecb63d9586e845d92
         BLS_MAP_TO_G1
         DUP
         DUP
         BLS_G1_ADD
         BLS_G1_ADD

         PUSHSLICE x7abd13983c76661a98659da83066c71bd6581baf20c82c825b007bf8057a258dc53f7a6d44fb6fdecb63d9586e845d92
         BLS_MAP_TO_G1
         PUSHINT 3
         BLS_G1_MUL

         BLS_G1_SUB
         BLS_G1_ISZERO",
    )
    .expect_success()
    .expect_stack(Stack::new().push(boolean!(true)));
}

#[test]
fn test_bls_g1_5() {
    test_case(
        "PUSHSLICE x7abd13983c76661a98659da83066c71bd6581baf20c82c825b007bf8057a258dc53f7a6d44fb6fdecb63d9586e845d92
         BLS_MAP_TO_G1 ; A
         DUP
         BLS_PUSHR
         PUSHINT 1
         SUB
         BLS_G1_MUL ; -A
         BLS_G1_ADD
         BLS_G1_ISZERO",
    )
    .expect_success()
    .expect_stack(Stack::new().push(boolean!(true)));
}

#[test]
fn test_bls_g1_6() {
    test_case(
        "BLS_G1_ZERO
         BLS_G1_ISZERO",
    )
    .expect_success()
    .expect_stack(Stack::new().push(boolean!(true)));
}

#[test]
fn test_bls_g1_7() {
    test_case(
        "PUSHSLICE x7abd13983c76661a98659da83066c71bd6581baf20c82c825b007bf8057a258dc53f7a6d44fb6fdecb63d9586e845d92
         BLS_G1_INGROUP",
    )
    .expect_success()
    .expect_stack(Stack::new().push(boolean!(false)));
}

#[test]
fn test_bls_g1_8() {
    test_case(
        "PUSHSLICE x7abd13983c76661a98659da83066c71bd6581baf20c82c825b007bf8057a258dc53f7a6d44fb6fdecb63d9586e845d92
         BLS_MAP_TO_G1
         BLS_G1_INGROUP",
    )
    .expect_success()
    .expect_stack(Stack::new().push(boolean!(true)));
}

// BLS_G1_MULTIEXP ( x_1 s_1 ... x_n s_n n - x_1*s_1+...+x_n*s_n)
#[test]
fn test_bls_g1_9() {
    test_case(
        "PUSHSLICE x7abd13983c76661a98659da83066c71bd6581baf20c82c825b007bf8057a258dc53f7a6d44fb6fdecb63d9586e845d92
         BLS_MAP_TO_G1
         PUSHINT 2
         
         PUSHSLICE x7abd13983c76661118659da83066c71bd6581baf20c82c825b007bf8057a258dc53f7a6d44fb6fdecb63d9586e845d92
         BLS_MAP_TO_G1
         PUSHINT 5
         
         PUSHSLICE x7abd13983c76661118659da83066c71bd658100020c82c825b007bf8057a258dc53f7a6d44fb6fdecb63d9586e845d92
         BLS_MAP_TO_G1
         PUSHINT 13537812947843
         
         PUSHINT 3
         BLS_G1_MULTIEXP
         
         PUSHSLICE x7abd13983c76661a98659da83066c71bd6581baf20c82c825b007bf8057a258dc53f7a6d44fb6fdecb63d9586e845d92
         BLS_MAP_TO_G1
         PUSHINT 2
         BLS_G1_MUL

         PUSHSLICE x7abd13983c76661118659da83066c71bd6581baf20c82c825b007bf8057a258dc53f7a6d44fb6fdecb63d9586e845d92
         BLS_MAP_TO_G1
         PUSHINT 5
         BLS_G1_MUL
         
         PUSHSLICE x7abd13983c76661118659da83066c71bd658100020c82c825b007bf8057a258dc53f7a6d44fb6fdecb63d9586e845d92
         BLS_MAP_TO_G1
         PUSHINT 13537812947843
         BLS_G1_MUL

         BLS_G1_ADD
         BLS_G1_ADD

         BLS_G1_SUB
         BLS_G1_ISZERO
         ",
    )
    .expect_success()
    .expect_stack(Stack::new().push(boolean!(true)));
}

#[test]
fn test_bls_g2_bad_data() {
    for instruction in ["BLS_G2_ADD", "BLS_G2_SUB", "BLS_G2_NEG"] {
        test_case(format!("
            PUSHSLICE xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff
            PUSHSLICE xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff
            {instruction}",
        ))
        .expect_failure(ExceptionCode::UnknownError);
    }
}

#[test]
fn test_bls_g2_bad_data2() {
    test_case("
        PUSHSLICE xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff
        PUSHINT 229
        BLS_G2_MUL",
    )
    .expect_failure(ExceptionCode::UnknownError);
}

#[test]
fn test_bls_g2_bad_data3() {
    test_case(
       "PUSHSLICE xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff
        PUSHINT 2
        PUSHSLICE xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff
        PUSHINT 5
        PUSHSLICE xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff
        PUSHINT 13537812947843
        PUSHINT 3
        BLS_G2_MULTIEXP
         ",
    )
    .expect_failure(ExceptionCode::UnknownError);
}

#[test]
fn test_bls_g2_1() {
    test_case(
        "PUSHSLICE xcce34c6322b8f3b455617a975aff8b6eaedf04fbae74a8890db6bc3fab0475b94cd8fbde0e1182ce6993afd56ed6e71919cae59c891923b4014ed9e42d9f0e1a779d9a7edb64f5e2fd600012805fc773b5092af5d2f0c6c0946ee9ad8394bf19
         BLS_MAP_TO_G2
         PUSHSLICE xcce34c6322b8f3b455617a975aff8b6eaedf04fbae74a8890db6bc3fab0475b94cd8fbde0e1182ce6993afd56ed6e71919cae59c891923b4014ed9e42d9f0e1a779d9a7edb64f5e2fd600012805fc773b5092af5d2f0c6c0946ee9ad8394bf19
         BLS_MAP_TO_G2
         BLS_G2_ADD
         PUSHSLICE xcce34c6322b8f3b455617a975aff8b6eaedf04fbae74a8890db6bc3fab0475b94cd8fbde0e1182ce6993afd56ed6e71919cae59c891923b4014ed9e42d9f0e1a779d9a7edb64f5e2fd600012805fc773b5092af5d2f0c6c0946ee9ad8394bf19
         BLS_MAP_TO_G2
         BLS_G2_SUB
        ",
    )
    .expect_success()
    .expect_stack(Stack::new()
        .push(create::slice(hex::decode(
            "8daf02bb3b3dca16883b251cb1a2eda65d10a29de23d06908a0e3c383b16a525242725a24900c973f1990d4a50de980500c0ede436577136c4c4e810e8308e1894b17a3ec112084cb3b6768e2d52b09da007f2071ec650776e0e0d27e2627e9480"
        ).unwrap()))
    );
}

#[test]
fn test_bls_g2_2() {
    test_case(
        "PUSHSLICE xcce34c6322b8f3b455617a975aff8b6eaedf04fbae74a8890db6bc3fab0475b94cd8fbde0e1182ce6993afd56ed6e71919cae59c891923b4014ed9e42d9f0e1a779d9a7edb64f5e2fd600012805fc773b5092af5d2f0c6c0946ee9ad8394bf19
         BLS_MAP_TO_G2
         BLS_G2_NEG
         PUSHSLICE xcce34c6322b8f3b455617a975aff8b6eaedf04fbae74a8890db6bc3fab0475b94cd8fbde0e1182ce6993afd56ed6e71919cae59c891923b4014ed9e42d9f0e1a779d9a7edb64f5e2fd600012805fc773b5092af5d2f0c6c0946ee9ad8394bf19
         BLS_MAP_TO_G2
         BLS_G2_ADD
         BLS_G2_ISZERO
        ",
    )
    .expect_success()
    .expect_stack(Stack::new().push(boolean!(true)));
}

#[test]
fn test_bls_g2_3() {
    test_case(
        "PUSHSLICE xcce34c6322b8f3b455617a975aff8b6eaedf04fbae74a8890db6bc3fab0475b94cd8fbde0e1182ce6993afd56ed6e71919cae59c891923b4014ed9e42d9f0e1a779d9a7edb64f5e2fd600012805fc773b5092af5d2f0c6c0946ee9ad8394bf19
         BLS_MAP_TO_G2
         BLS_G2_NEG

         PUSHSLICE xcce34c6322b8f3b455617a975aff8b6eaedf04fbae74a8890db6bc3fab0475b94cd8fbde0e1182ce6993afd56ed6e71919cae59c891923b4014ed9e42d9f0e1a779d9a7edb64f5e2fd600012805fc773b5092af5d2f0c6c0946ee9ad8394bf19
         BLS_MAP_TO_G2
         PUSHINT -1
         BLS_G2_MUL

         BLS_G2_SUB
         BLS_G2_ISZERO
        ",
    )
    .expect_success()
    .expect_stack(Stack::new().push(boolean!(true)));
}

#[test]
fn test_bls_g2_4() {
    test_case(
        "PUSHSLICE xcce34c6322b8f3b455617a975aff8b6eaedf04fbae74a8890db6bc3fab0475b94cd8fbde0e1182ce6993afd56ed6e71919cae59c891923b4014ed9e42d9f0e1a779d9a7edb64f5e2fd600012805fc773b5092af5d2f0c6c0946ee9ad8394bf19
         BLS_MAP_TO_G2
         DUP
         DUP
         BLS_G2_ADD
         BLS_G2_ADD

         PUSHSLICE xcce34c6322b8f3b455617a975aff8b6eaedf04fbae74a8890db6bc3fab0475b94cd8fbde0e1182ce6993afd56ed6e71919cae59c891923b4014ed9e42d9f0e1a779d9a7edb64f5e2fd600012805fc773b5092af5d2f0c6c0946ee9ad8394bf19
         BLS_MAP_TO_G2
         PUSHINT 3
         BLS_G2_MUL

         BLS_G2_SUB
         BLS_G2_ISZERO",
    )
    .expect_success()
    .expect_stack(Stack::new().push(boolean!(true)));
}

#[test]
fn test_bls_g2_5() {
    test_case(
        "PUSHSLICE xcce34c6322b8f3b455617a975aff8b6eaedf04fbae74a8890db6bc3fab0475b94cd8fbde0e1182ce6993afd56ed6e71919cae59c891923b4014ed9e42d9f0e1a779d9a7edb64f5e2fd600012805fc773b5092af5d2f0c6c0946ee9ad8394bf19
         BLS_MAP_TO_G2 ; A
         DUP
         BLS_PUSHR
         PUSHINT 1
         SUB
         BLS_G2_MUL ; -A
         BLS_G2_ADD
         BLS_G2_ISZERO",
    )
    .expect_success()
    .expect_stack(Stack::new().push(boolean!(true)));
}

#[test]
fn test_bls_g2_6() {
    test_case(
        "BLS_G2_ZERO
         BLS_G2_ISZERO",
    )
    .expect_success()
    .expect_stack(Stack::new().push(boolean!(true)));
}

#[test]
fn test_bls_g2_7() {
    test_case(
        "PUSHSLICE xcce34c6322b8f3b455617a975aff8b6eaedf04fbae74a8890db6bc3fab0475b94cd8fbde0e1182ce6993afd56ed6e71919cae59c891923b4014ed9e42d9f0e1a779d9a7edb64f5e2fd600012805fc773b5092af5d2f0c6c0946ee9ad8394bf19
         BLS_G2_INGROUP",
    )
    .expect_success()
    .expect_stack(Stack::new().push(boolean!(false)));
}

#[test]
fn test_bls_g2_8() {
    test_case(
        "PUSHSLICE xcce34c6322b8f3b455617a975aff8b6eaedf04fbae74a8890db6bc3fab0475b94cd8fbde0e1182ce6993afd56ed6e71919cae59c891923b4014ed9e42d9f0e1a779d9a7edb64f5e2fd600012805fc773b5092af5d2f0c6c0946ee9ad8394bf19
         BLS_MAP_TO_G2
         BLS_G2_INGROUP",
    )
    .expect_success()
    .expect_stack(Stack::new().push(boolean!(true)));
}

// BLS_G2_MULTIEXP ( x_1 s_1 ... x_n s_n n - x_1*s_1+...+x_n*s_n)
#[test]
fn test_bls_g2_9() {
    test_case(
        "PUSHSLICE xcce34c6322b8f3b455617a975aff8b6eaedf04fbae74a8890db6bc3fab0475b94cd8fbde0e1182ce6993afd56ed6e71919cae59c891923b4014ed9e42d9f0e1a779d9a7edb64f5e2fd600012805fc773b5092af5d2f0c6c0946ee9ad8394bf19
         BLS_MAP_TO_G2
         PUSHINT 2
         
         PUSHSLICE x7abd13983c76661118659da83066c71bd6581baf20c82c825b007bf8057a258dc53f7a6d44fb6fdecb63d9586e845d927abd13983c76661118659da83066c71bd6581baf20c82c825b007bf8057a258dc53f7a6d44fb6fdecb63d9586e845d92
         BLS_MAP_TO_G2
         PUSHINT 5
         
         PUSHSLICE x7abd13983c76661118659da83066c71bd658100020c82c825b007bf8057a258dc53f7a6d44fb6fdecb63d9586e845d927abd13983c76661118659da83066c71bd658100020c82c825b007bf8057a258dc53f7a6d44fb6fdecb63d9586e845d92
         BLS_MAP_TO_G2
         PUSHINT 13537812947843
         
         PUSHINT 3
         BLS_G2_MULTIEXP
         
         PUSHSLICE xcce34c6322b8f3b455617a975aff8b6eaedf04fbae74a8890db6bc3fab0475b94cd8fbde0e1182ce6993afd56ed6e71919cae59c891923b4014ed9e42d9f0e1a779d9a7edb64f5e2fd600012805fc773b5092af5d2f0c6c0946ee9ad8394bf19
         BLS_MAP_TO_G2
         PUSHINT 2
         BLS_G2_MUL

         PUSHSLICE x7abd13983c76661118659da83066c71bd6581baf20c82c825b007bf8057a258dc53f7a6d44fb6fdecb63d9586e845d927abd13983c76661118659da83066c71bd6581baf20c82c825b007bf8057a258dc53f7a6d44fb6fdecb63d9586e845d92
         BLS_MAP_TO_G2
         PUSHINT 5
         BLS_G2_MUL
         
         PUSHSLICE x7abd13983c76661118659da83066c71bd658100020c82c825b007bf8057a258dc53f7a6d44fb6fdecb63d9586e845d927abd13983c76661118659da83066c71bd658100020c82c825b007bf8057a258dc53f7a6d44fb6fdecb63d9586e845d92
         BLS_MAP_TO_G2
         PUSHINT 13537812947843
         BLS_G2_MUL

         BLS_G2_ADD
         BLS_G2_ADD

         BLS_G2_SUB
         BLS_G2_ISZERO
         ",
    )
    .expect_success()
    .expect_stack(Stack::new().push(boolean!(true)));
}

#[test]
fn test_bls_pairing() {
    test_case(
        "; x * G1
         PUSHSLICE x7abd13983c76661a98659da83066c71bd6581baf20c82c825b007bf8057a258dc53f7a6d44fb6fdecb63d9586e845d92
         BLS_MAP_TO_G1
         PUSHINT 2
         BLS_G1_MUL
        
         ; G2
         PUSHSLICE xcce34c6322b8f3b455617a975aff8b6eaedf04fbae74a8890db6bc3fab0475b94cd8fbde0e1182ce6993afd56ed6e71919cae59c891923b4014ed9e42d9f0e1a779d9a7edb64f5e2fd600012805fc773b5092af5d2f0c6c0946ee9ad8394bf19
         BLS_MAP_TO_G2
         PUSHINT 1234567890
         BLS_G2_MUL

         ; G1
         PUSHSLICE x7abd13983c76661a98659da83066c71bd6581baf20c82c825b007bf8057a258dc53f7a6d44fb6fdecb63d9586e845d92
         BLS_MAP_TO_G1
         PUSHINT 1234567890
         BLS_G1_MUL

         ; x * G2
         PUSHSLICE xcce34c6322b8f3b455617a975aff8b6eaedf04fbae74a8890db6bc3fab0475b94cd8fbde0e1182ce6993afd56ed6e71919cae59c891923b4014ed9e42d9f0e1a779d9a7edb64f5e2fd600012805fc773b5092af5d2f0c6c0946ee9ad8394bf19
         BLS_MAP_TO_G2
         PUSHINT 2
         BLS_G2_MUL
         BLS_G2_NEG
         
         PUSHINT 2
         BLS_PAIRING"
    )
    .expect_success()
    .expect_stack(Stack::new().push(boolean!(true)));
}
