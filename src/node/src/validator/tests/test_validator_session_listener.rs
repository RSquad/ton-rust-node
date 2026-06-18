use super::*;
use ton_block::{
    BlockSignatures, BlockSignaturesVariant, Ed25519KeyOption, UInt256, ZeroizingBytes,
};

#[test]
fn test_on_applied_top_action_preserves_queue_order() {
    let (listener, mut receiver) =
        ValidatorSessionListener::create(UInt256::default(), ShardIdent::masterchain());
    let applied_top =
        BlockIdExt::with_params(ShardIdent::masterchain(), 7, UInt256::rand(), UInt256::rand());

    listener
        .queue_sender()
        .send(ValidationAction::OnAppliedTop { applied_top: applied_top.clone() })
        .expect("send applied-top");
    listener
        .queue_sender()
        .send(ValidationAction::OnBlockSkipped { round: 11 })
        .expect("send skip");

    match receiver.try_recv().expect("first queued action") {
        ValidationAction::OnAppliedTop { applied_top: queued } => {
            assert_eq!(queued, applied_top);
        }
        other => panic!("unexpected first action: {}", other),
    }

    match receiver.try_recv().expect("second queued action") {
        ValidationAction::OnBlockSkipped { round } => assert_eq!(round, 11),
        other => panic!("unexpected second action: {}", other),
    }
}

#[test]
fn test_on_block_finalized_enqueues_explicit_block_identity() {
    let (listener, mut receiver) =
        ValidatorSessionListener::create(UInt256::default(), ShardIdent::masterchain());

    let block_id =
        BlockIdExt::with_params(ShardIdent::masterchain(), 42, UInt256::rand(), UInt256::rand());
    let source = Ed25519KeyOption::<ZeroizingBytes>::generate().expect("generate key");
    let source_info = BlockSourceInfo {
        source,
        priority: consensus_common::BlockCandidatePriority {
            round: 10,
            priority: 0,
            first_block_round: 10,
        },
    };
    let root_hash = UInt256::rand();
    let file_hash = UInt256::rand();
    let data = consensus_common::ConsensusCommonFactory::create_block_payload(vec![1, 2, 3, 4]);
    let signatures = BlockSignaturesVariant::Ordinary(BlockSignatures::default());

    listener.on_block_finalized(
        block_id.clone(),
        source_info,
        root_hash.clone(),
        file_hash.clone(),
        data.clone(),
        signatures,
        Vec::new(),
    );

    match receiver.try_recv().expect("queued finalized action") {
        ValidationAction::OnBlockFinalized(finalized) => {
            assert_eq!(finalized.block_id, block_id);
            assert_eq!(finalized.source_info.priority.round, 10);
            assert_eq!(finalized.root_hash, root_hash);
            assert_eq!(finalized.file_hash, file_hash);
            assert_eq!(finalized.data.data(), data.data());
            match finalized.signatures {
                BlockSignaturesVariant::Ordinary(_) => {}
                other => panic!("unexpected signatures variant in queued action: {:?}", other),
            }
        }
        other => panic!("unexpected queued action: {}", other),
    }
}
