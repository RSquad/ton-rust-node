/*
 * Copyright (C) 2019-2021 TON Labs. All Rights Reserved.
 * Modifications Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This file has been modified from its original version.
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use super::*;
use crate::{
    block_parser::{reducers::JsonFieldsReducer, MINTER_ADDRESS},
    NoTrace, ParsedBlock,
};
use serde_json::Map;
use std::{collections::HashMap, fs::read, path::Path};
use ton_block::{read_single_root_boc, Block, GetRepresentationHash, InMsg, OutMsg, UInt256};

#[derive(Default)]
pub struct ParseOptions {
    mc_seq_no: Option<u32>,
    file_hash: Option<UInt256>,
    blocks: Option<EntryConfig<JsonFieldsReducer>>,
    transactions: Option<EntryConfig<JsonFieldsReducer>>,
    messages: Option<EntryConfig<JsonFieldsReducer>>,
}

impl ParseOptions {
    fn mc_seq_no(self, seq_no: u32) -> Self {
        Self { mc_seq_no: Some(seq_no), ..self }
    }

    fn sharding(self, blocks: u32, transactions: u32, messages: u32) -> Self {
        fn config(depth: u32) -> Option<EntryConfig<JsonFieldsReducer>> {
            Some(EntryConfig { reducer: None, sharding_depth: Some(depth) })
        }
        Self {
            blocks: config(blocks),
            transactions: config(transactions),
            messages: config(messages),
            ..self
        }
    }

    fn file_hash(self, file_hash: UInt256) -> Self {
        Self { file_hash: Some(file_hash), ..self }
    }
}

fn reducer(config: &str) -> Option<EntryConfig<JsonFieldsReducer>> {
    Some(EntryConfig {
        reducer: Some(JsonFieldsReducer::with_config(config).unwrap()),
        sharding_depth: None,
    })
}

fn parse_block(
    file_rel_path: &str,
    options: Option<ParseOptions>,
) -> (Vec<u8>, UInt256, ParsedBlock) {
    let in_path = Path::new("src/tests/data").join(file_rel_path);
    let boc = read(in_path.clone()).unwrap_or_else(|_| panic!("Error reading file {:?}", in_path));
    let cell = read_single_root_boc(&boc).expect("Error deserializing single root BOC");

    let block = Block::construct_from_cell(cell.clone()).unwrap();
    let info = block.read_info().unwrap();
    let file_hash = options
        .as_ref()
        .and_then(|x| x.file_hash.clone())
        .unwrap_or_else(|| UInt256::calc_file_hash(&boc));
    let id = BlockIdExt::with_params(
        info.shard().clone(),
        info.seq_no(),
        block.hash().unwrap().clone(),
        file_hash,
    );
    let mc_seq_no = options.as_ref().and_then(|x| x.mc_seq_no);
    let (blocks, transactions, messages) =
        options.map(|x| (x.blocks, x.transactions, x.messages)).unwrap_or((None, None, None));
    fn entry_config(
        opt: Option<EntryConfig<JsonFieldsReducer>>,
    ) -> Option<EntryConfig<JsonFieldsReducer>> {
        Some(opt.unwrap_or(EntryConfig { reducer: None, sharding_depth: None }))
    }

    let parser = BlockParser::<NoTrace, JsonFieldsReducer>::new(
        BlockParserConfig {
            blocks: entry_config(blocks),
            transactions: entry_config(transactions),
            messages: entry_config(messages),
            accounts: None,
            proofs: None,
            max_account_bytes_size: None,
            is_node_se: false,
        },
        None,
    );
    let parsed = parser
        .parse(
            ParsingBlock {
                id: &id,
                block: &block,
                root: &cell,
                shard_state: None,
                data: &boc,
                mc_seq_no,
                proof: None,
            },
            false,
        )
        .unwrap();
    (boc, cell.repr_hash(), parsed)
}

#[test]
fn test_transaction_code_hash() {
    println!("MA: {:?}", MINTER_ADDRESS);
    let (_, _, parsed) =
        parse_block("89ED400A43E76664437EFC9C79B84AC387493A9EE5E789338FF71C25F54218BE.boc", None);
    fn has_code_hash(entry: &ParsedEntry, field: &str) -> bool {
        entry.body.get(field).and_then(|x| x.as_str()).is_some_and(|x| !x.is_empty())
    }

    for tr in &parsed.transactions {
        assert!(has_code_hash(tr, "code_hash"), "transaction should have code hash");
    }
    for msg in &parsed.messages {
        let has_src_code_hash = has_code_hash(msg, "src_code_hash");
        let has_dst_code_hash = has_code_hash(msg, "dst_code_hash");
        assert!(has_src_code_hash || has_dst_code_hash, "message should have src or dst code hash");
    }
    assert_eq!(
        parsed.block.unwrap().id,
        "c6eb57829560dcba3fdc9600cbf1cfe8c9f8e1f89a4a9b085f52cb06c4996784"
    );
}

#[test]
fn test_parse_block() {
    //crate::init_logger(None);
    let (raw_block, block_id, parsed) =
        parse_block("3FAFAAB7B5D17E439CD9DDEF7EEC3C3BC9D28E7C181BA374218E13599F8F6657.boc", None);

    let block: Block = Block::construct_from_bytes(&raw_block).unwrap();
    let block_extra = block.read_extra().unwrap();

    assert_eq!(parsed.messages.len(), 2);
    assert_eq!(parsed.transactions.len(), 2);

    // serialization changes often so check only id's and records count
    let check_parsed_entry = |entry: ParsedEntry, id: UInt256| {
        let value = &entry.body;
        assert_eq!(value["id"], serde_json::Value::from(id.to_hex_string()));
        assert!(value.get("chain_order").is_none());
        // message key must be valid JSON, so quotation marks needed
        assert_eq!(entry.id, id.to_hex_string());
        assert_eq!(entry.partition, None);
    };

    // Block
    check_parsed_entry(parsed.block.unwrap().clone(), block_id.clone());

    let mut messages: HashMap<String, ParsedEntry> =
        parsed.messages.into_iter().map(|parsed| (parsed.id.clone(), parsed)).collect();
    // Messages
    block_extra
        .read_in_msg_descr()
        .unwrap()
        .iterate_objects(&mut |msg: InMsg| {
            if let InMsg::External(_) = msg {
                let id = msg.message_cell().unwrap().repr_hash();
                check_parsed_entry(messages.remove(&id.to_hex_string()).unwrap(), id);
            }
            Ok(true)
        })
        .unwrap();
    block_extra
        .read_out_msg_descr()
        .unwrap()
        .iterate_objects(&mut |msg: OutMsg| {
            match msg {
                OutMsg::External(_) | OutMsg::Immediate(_) | OutMsg::New(_) => {
                    if let Some(cell) = msg.message_cell().unwrap() {
                        let id = cell.repr_hash();
                        check_parsed_entry(messages.remove(&id.to_hex_string()).unwrap(), id);
                    }
                }
                _ => {}
            };
            Ok(true)
        })
        .unwrap();
    assert_eq!(messages.len(), 0);

    let mut transactions = HashMap::<UInt256, ParsedEntry>::from_iter(
        parsed.transactions.into_iter().map(|entry| (entry.id.parse().unwrap(), entry)),
    );

    // Transactions
    block_extra
        .read_account_blocks()
        .unwrap()
        .iterate_objects(&mut |account_block: AccountBlock| {
            account_block
                .transactions()
                .iterate_slices(&mut |_, transaction_slice: SliceData| {
                    let cell = transaction_slice.reference(0).unwrap();
                    let id = cell.repr_hash();
                    check_parsed_entry(transactions.remove(&id).unwrap(), id);
                    Ok(true)
                })
                .unwrap();
            Ok(true)
        })
        .unwrap();
    assert_eq!(transactions.len(), 0);
}

#[test]
fn test_parse_mc_block() {
    //crate::init_logger(None);
    let (_, _, parsed) =
        parse_block("a9c07ece30e9b4fc446b8262a206dcd25a7c5c0b47f38fd3264d3031d739f9c3.boc", None);

    assert_eq!(parsed.messages.len(), 1);
    assert_eq!(parsed.transactions.len(), 5);

    assert!(parsed.block.unwrap().body.get("chain_order").is_none());
    assert!(parsed.transactions[0].body.get("chain_order").is_none());
    assert!(parsed.messages[0].body.get("chain_order").is_none());
}

fn check_chain_order(body: &Map<String, Value>, id: &str, chain_order: &str) {
    assert_eq!(body["id"], Value::from(id));
    assert_eq!(body["chain_order"], Value::from(chain_order));
}

fn check_msg_chain_order(
    body: &Map<String, Value>,
    id: &str,
    src_chain_order: Option<&str>,
    dst_chain_order: Option<&str>,
) {
    println!("{:?}", body);
    assert_eq!(body["id"], Value::from(id));
    if let Some(order) = src_chain_order {
        assert_eq!(body["src_chain_order"], Value::from(order));
    } else {
        assert!(body.get("src_chain_order").is_none());
    }
    if let Some(order) = dst_chain_order {
        assert_eq!(body["dst_chain_order"], Value::from(order));
    } else {
        assert!(body.get("dst_chain_order").is_none());
    }
}

#[test]
fn test_mc_chain_order() {
    let (_, block_id, parsed) = parse_block(
        "de8d870c6944248b89ed7d510a99470d5aae3a6df918a7ef6fdc8b246d71ca49.boc",
        Some(ParseOptions::default().mc_seq_no(8631080)),
    );

    check_chain_order(&parsed.block.unwrap().body, &block_id.to_hex_string(), "583b328m");

    let mut transaction_orders = vec![
        ("110251141556a08a7d5718bfc6c3d9b9811f2cdf4ff3b156ac3a6f57b8201391", "583b328m00"),
        ("589e6b6e5052dbefd525a5589dc50ec789c8b9241cfc2b258c222643d4657527", "583b328m01"),
        ("f84d8a663284c28c7ccafc7db4f7ebc32b1b9cd25ce029ab9be940ba1ba6ec08", "583b328m02"),
        ("0c9c38178cb4c931c715b1909d4e7f96aafc427e05a2c0d0c2bdcf75ade9e8a0", "583b328m03"),
        ("3fc28a93c16cea6e0bdc3b0eba497e911cc9b7d421e8284314db2b14a730c1da", "583b328m04"),
        ("2b0c0f1de578d352190328a4bfb2a1f8875c1f05a837e8abc154993c152bd240", "583b328m05"),
    ];

    for transaction in parsed.transactions.into_iter().rev() {
        let (id, order) = transaction_orders.pop().unwrap();
        check_chain_order(&transaction.body, id, order);
    }

    let message_orders = HashMap::<&str, (Option<&str>, Option<&str>)>::from_iter([
        (
            "a72d1215acf9434ad7e72027ea4020ead898f4d443a23c9f5ce06bc88abbb477",
            (None, Some("583b328m0200")),
        ),
        (
            "ba68ba73d1c523f7e986cd1a5594dad0e80a73781ebf95dded7345045c4b8b30",
            (Some("583b328m0201"), None),
        ),
        (
            "4d532563b2ceddd769496bb66445714f70ef5b49a626eb521cbfde261c8462f5",
            (None, Some("583b328m0300")),
        ),
    ]);
    assert_eq!(message_orders.len(), parsed.messages.len());
    for message in parsed.messages {
        let (src_order, dst_order) = message_orders.get(&message.id.as_str()).unwrap();
        check_msg_chain_order(&message.body, &message.id, *src_order, *dst_order);
    }
}

#[test]
fn test_wc_chain_order() {
    let (_, block_id, parsed) = parse_block(
        "3FAFAAB7B5D17E439CD9DDEF7EEC3C3BC9D28E7C181BA374218E13599F8F6657.boc",
        Some(ParseOptions::default().mc_seq_no(123)),
    );

    check_chain_order(&parsed.block.unwrap().body, &block_id.to_hex_string(), "17b0054702f3110");

    let mut transaction_orders = vec![
        ("8d512de1f07239705972edf4a1d89837d9c2c8968bfec16b721ced94481a9058", "17b0054702f311000"),
        ("f0f3750f451afbc6e5b0d93fc3446a20b2fcea24f948997b33aa8e9a5f99908b", "17b0054702f311001"),
    ];

    for transaction in parsed.transactions.into_iter().rev() {
        let (id, order) = transaction_orders.pop().unwrap();
        check_chain_order(&transaction.body, id, order);
    }

    let message_orders = HashMap::<&str, (Option<&str>, Option<&str>)>::from_iter([
        (
            "d0eb54b4ab2dc85e8e2c7044ae4047a9d10d9e2a13a6bc4055c1f15a67db5d80",
            (None, Some("17b0054702f31100000")),
        ),
        (
            "e3b9e21c18c77c02a5e21445de1a69cdb01a8e1dd84601b1ae31b8e568b60153",
            (Some("17b0054702f31100001"), Some("17b0054702f31100100")),
        ),
    ]);

    assert_eq!(message_orders.len(), parsed.messages.len());
    for message in parsed.messages {
        let (src_order, dst_order) = message_orders.get(&message.id.as_str()).unwrap();
        check_msg_chain_order(&message.body, &message.id, *src_order, *dst_order);
    }
}

#[test]
fn test_immediate_chain_order() {
    let (_, _, parsed) = parse_block(
        "c594aec0b3e52b4177e90d3bf82542151b0edc3e5d86d745689ec25f3d943624.boc",
        Some(ParseOptions::default().mc_seq_no(123)),
    );

    let message_orders = HashMap::<&str, (Option<&str>, Option<&str>)>::from_iter([
        (
            "1a8186bdc9e0d799f7d732e2511a7b63c92db266fd26b3ccb1341b54df7c1b55",
            (None, Some("17b0039f181100000")),
        ),
        (
            "a6e37934b3afa3a33c7f119637308fcdd7d6f3ff87f91b5d73ed5e7e8f6220c1",
            (Some("17b0039f181100001"), Some("17b0039f181100200")),
        ),
        (
            "7311adad273155b2a30ca8e491d3f159cb8c4a15dd404f04a2ae54cb95e37621",
            (Some("17b0039f181100002"), Some("17b0039f181100100")),
        ),
    ]);

    assert_eq!(message_orders.len(), parsed.messages.len());
    for message in parsed.messages {
        let (src_order, dst_order) = message_orders.get(&message.id.as_str()).unwrap();
        check_msg_chain_order(&message.body, &message.id, *src_order, *dst_order);
    }
}

#[test]
fn test_wc_sharding() {
    let (_, _, parsed) = parse_block(
        "3FAFAAB7B5D17E439CD9DDEF7EEC3C3BC9D28E7C181BA374218E13599F8F6657.boc",
        Some(ParseOptions::default().sharding(1, 0, 8).mc_seq_no(123)),
    );
    assert_eq!(parsed.block.unwrap().partition, Some(1));

    for transaction in parsed.transactions {
        assert_eq!(transaction.partition, None);
    }

    assert_eq!(
        HashMap::from_iter([
            (
                "d0eb54b4ab2dc85e8e2c7044ae4047a9d10d9e2a13a6bc4055c1f15a67db5d80".to_owned(),
                Some(12)
            ),
            (
                "e3b9e21c18c77c02a5e21445de1a69cdb01a8e1dd84601b1ae31b8e568b60153".to_owned(),
                Some(12)
            ),
        ]),
        parsed
            .messages
            .into_iter()
            .map(|msg| (msg.id, msg.partition))
            .collect::<HashMap<String, Option<u32>>>(),
    );
}

#[test]
fn test_mc_sharding() {
    let (_, _, parsed) = parse_block(
        "a9c07ece30e9b4fc446b8262a206dcd25a7c5c0b47f38fd3264d3031d739f9c3.boc",
        Some(ParseOptions::default().sharding(5, 3, 4).mc_seq_no(326186)),
    );
    assert_eq!(parsed.block.unwrap().partition, Some(21));

    assert_eq!(
        vec![
            (
                "27e44b75d0e8861adc1ca8909f4ac17ede02bd56af9c6a4ab950230c5c3579c0".to_owned(),
                Some(0)
            ),
            (
                "0fc37da74bc1f826e81e1f97b4e997143e9147d94288db89472073cf87b020d0".to_owned(),
                Some(1)
            ),
            (
                "004fc2db789bea0321b76a89e0864bf630cc0210713383147fcf1ac81eac6400".to_owned(),
                Some(1)
            ),
            (
                "caf7d8cec0da5ad17771bd43f52dd9a2c4ad49b232cf2680ab47ac4756f2d74c".to_owned(),
                Some(0)
            ),
            (
                "9e2e6d9ae16449556223dbc14584d647289bde734c9cd6f263efc0c338391b2c".to_owned(),
                Some(2)
            ),
        ],
        parsed
            .transactions
            .into_iter()
            .map(|tr| (tr.id, tr.partition))
            .collect::<Vec<(String, Option<u32>)>>(),
    );

    assert_eq!(
        vec![(
            "f0c6b0b8f1aa2f0ad7083f8e1dad96e475311a33c44c2aabe2605e2d994c9316".to_owned(),
            Some(0)
        ),],
        parsed
            .messages
            .into_iter()
            .map(|msg| (msg.id, msg.partition))
            .collect::<Vec<(String, Option<u32>)>>(),
    );
}

#[test]
fn test_ext_in_sharding() {
    let (_, _, parsed) = parse_block(
        "6ce37a48b76f9ab9a5b33b727baf3e19da18a7bdee1cf3242ddb2a79c20715e4.boc",
        Some(ParseOptions::default().sharding(5, 3, 4).mc_seq_no(123)),
    );
    assert_eq!(parsed.block.unwrap().partition, Some(13));

    assert_eq!(
        HashMap::from_iter([
            (
                "3479e84f2a5e93f1d1192487e32b9251c1201a2a94ea1220d878687837201297".to_owned(),
                Some(15)
            ),
            (
                "a63c39b44d76f81f2b2f590d466a0354816a950dd6609a2c23fe76dceff8856d".to_owned(),
                Some(15)
            ),
            (
                "f6b8d72a4a620bfd030f5fa2a44651729e8f7a49f14c98ff06ed9bd4008bb6d9".to_owned(),
                Some(2)
            ),
            (
                "7c10a3c9f8c896a10d133da7bcb597839866a87ca7cff15c9b41b7befed84403".to_owned(),
                Some(2)
            ),
        ]),
        parsed
            .messages
            .into_iter()
            .map(|msg| (msg.id, msg.partition))
            .collect::<HashMap<String, Option<u32>>>(),
    );
}

#[test]
fn test_ext_out_sharding() {
    let (_, _, parsed) = parse_block(
        "558651b80d5361fd7f31882d4df90bf8e3c0c58422684e752a47c6b57b7be62c.boc",
        Some(ParseOptions::default().sharding(5, 3, 4).mc_seq_no(41)),
    );
    assert_eq!(parsed.block.unwrap().partition, Some(10));

    for msg in &parsed.messages {
        println!("{:#}", Value::Object(msg.body.clone()));
    }

    assert_eq!(
        HashMap::from_iter([
            (
                "f76c734907a83a3d8bec3f1ac469cf7c5c0593efc7bea5b65434b8d3f4092a5e".to_owned(),
                Some(0)
            ),
            (
                "a37a54b949cf8762cad66326f847f9907f61471e9af0f18285cf0730aaaa7859".to_owned(),
                Some(15)
            ),
            (
                "722133a7ed751599fc4fa8d37ada21453fcc5c078a842d57bf28644ef43f333f".to_owned(),
                Some(0)
            ),
            (
                "736d23ac19a3ee425bc4e50b60b0e43a46b21a181008f4c0103b54034dbd9469".to_owned(),
                Some(0)
            ),
            (
                "33f92622387f67117d87a919c827ceecac21d914563c2ce228437ddf984933fc".to_owned(),
                Some(0)
            ),
            (
                "1653c2f14e2d09f199763911f82bc66479f1193963c1555a0a3f5f8656dc3e6d".to_owned(),
                Some(0)
            ),
            (
                "e52ea01215504daba82b05e45839127e7946bfb6c65130fc61fc2aeb4437982a".to_owned(),
                Some(0)
            ),
            (
                "4b74b0b3ea3442bbc8db5429072a445d7a966c179faa449216afdff8710f17b3".to_owned(),
                Some(15)
            ),
        ]),
        parsed
            .messages
            .into_iter()
            .map(|msg| (msg.id, msg.partition))
            .collect::<HashMap<String, Option<u32>>>(),
    );
}

fn check_field(body: &Map<String, Value>, pointer: &str, value: &Value) {
    assert_eq!(Value::Object(body.clone()).pointer(pointer), Some(value));
}

#[test]
fn test_file_hash() {
    let (_, _, parsed) =
        parse_block("558651b80d5361fd7f31882d4df90bf8e3c0c58422684e752a47c6b57b7be62c.boc", None);

    check_field(
        &parsed.block.unwrap().body,
        "/file_hash",
        &"11d25878310eba95c7ba89a4f3db4e84261944fa19a9b524c627ca60cd4ce379".into(),
    );

    let (_, _, parsed) = parse_block(
        "558651b80d5361fd7f31882d4df90bf8e3c0c58422684e752a47c6b57b7be62c.boc",
        Some(ParseOptions::default().mc_seq_no(41).file_hash(UInt256::default())),
    );

    check_field(
        &parsed.block.unwrap().body,
        "/file_hash",
        &"0000000000000000000000000000000000000000000000000000000000000000".into(),
    );
}

#[test]
fn test_reduce_config() {
    let (_, _, parsed) = parse_block(
        "558651b80d5361fd7f31882d4df90bf8e3c0c58422684e752a47c6b57b7be62c.boc",
        Some(ParseOptions {
            blocks: reducer("{ id seq_no }"),
            messages: reducer("{ id dst }"),
            ..Default::default()
        }),
    );
    let parsed_block = parsed.block.unwrap().body;
    let test_block = serde_json::json!({
        "id": parsed_block["id"],
        "seq_no": parsed_block["seq_no"],
    });
    assert_eq!(Value::Object(parsed_block), test_block);

    for msg in parsed.messages {
        let mut msg = msg.body;
        assert!(msg.remove("id").is_some());
        msg.remove("dst");
        assert_eq!(msg.len(), 0);
    }

    for trans in parsed.transactions {
        assert!(!trans.body["now"].is_null());
    }
}

#[test]
fn test_transaction_id_in_msg() {
    let (_, _, parsed) =
        parse_block("3FAFAAB7B5D17E439CD9DDEF7EEC3C3BC9D28E7C181BA374218E13599F8F6657.boc", None);

    let message_ethalons = HashMap::<&str, Value>::from_iter([
        (
            "e3b9e21c18c77c02a5e21445de1a69cdb01a8e1dd84601b1ae31b8e568b60153",
            serde_json::json!({
                "src_transaction_id": "8d512de1f07239705972edf4a1d89837d9c2c8968bfec16b721ced94481a9058",
                "dst_transaction_id": "f0f3750f451afbc6e5b0d93fc3446a20b2fcea24f948997b33aa8e9a5f99908b",
            }),
        ),
        (
            "d0eb54b4ab2dc85e8e2c7044ae4047a9d10d9e2a13a6bc4055c1f15a67db5d80",
            serde_json::json!({
                "dst_transaction_id": "8d512de1f07239705972edf4a1d89837d9c2c8968bfec16b721ced94481a9058",
            }),
        ),
    ]);

    for message in parsed.messages {
        let msg_ethalon = message_ethalons.get(message.id.as_str()).unwrap();
        assert_eq!(
            message.body.get("src_transaction_id").unwrap_or(&Value::Null).clone(),
            msg_ethalon["src_transaction_id"]
        );
        assert_eq!(
            message.body.get("dst_transaction_id").unwrap_or(&Value::Null).clone(),
            msg_ethalon["dst_transaction_id"]
        );
    }
}
