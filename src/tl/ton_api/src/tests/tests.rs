/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::{serialize_boxed, Constructor, Deserializer};

#[test]
fn test_enum_serialization() {
    let msg = crate::ton::adnl::Message::Adnl_Message_Nop;
    serialize_boxed(&msg).unwrap();
}

#[test]
fn test_deserialize_vector_with_big_struct() {
    let max_count = 1i32 << 20;
    let constructor_tag =
        crate::ton::validator_session::statsround::StatsRound::constructor_const();
    let mut serialized = constructor_tag.to_le_bytes().to_vec();
    serialized.extend_from_slice(&[0; 8]);
    serialized.extend_from_slice(&max_count.to_le_bytes());
    let mut binding = serialized.as_slice();
    let mut de = Deserializer::new(&mut binding);
    de.read_boxed::<crate::ton::validator_session::StatsRound>().unwrap_err();
}

#[test]
fn test_check_all_structs_with_vecs() {
    let sizes = &[
        ("crate::ton::int", std::mem::size_of::<crate::ton::int>()),
        ("crate::ton::long", std::mem::size_of::<crate::ton::long>()),
        ("crate::ton::ton_node::shardid::ShardId", std::mem::size_of::<crate::ton::ton_node::shardid::ShardId>()),
        ("crate::ton::rwallet::limit::Limit", std::mem::size_of::<crate::ton::rwallet::limit::Limit>()),
        ("crate::ton::extracurrency::ExtraCurrency", std::mem::size_of::<crate::ton::extracurrency::ExtraCurrency>()),
        ("crate::ton::string", std::mem::size_of::<crate::ton::string>()),
        ("crate::ton::secureString", std::mem::size_of::<crate::ton::secureString>()),
        ("crate::ton::bytes", std::mem::size_of::<crate::ton::bytes>()),
        ("crate::ton::int256", std::mem::size_of::<crate::ton::int256>()),
        ("crate::ton::consensus::simplex::VoteSignature", std::mem::size_of::<crate::ton::consensus::simplex::VoteSignature>()),
        ("crate::ton::storage::PriorityAction", std::mem::size_of::<crate::ton::storage::PriorityAction>()),
        ("crate::ton::tvm::StackEntry", std::mem::size_of::<crate::ton::tvm::StackEntry>()),
        ("crate::ton::liteserver::desc_v2::shardinfo::ShardInfo", std::mem::size_of::<crate::ton::liteserver::desc_v2::shardinfo::ShardInfo>()),
        ("crate::ton::validator_session::statsround::StatsRound", std::mem::size_of::<crate::ton::validator_session::statsround::StatsRound>()),
        ("crate::ton::adnl::stats::ippackets::IpPackets", std::mem::size_of::<crate::ton::adnl::stats::ippackets::IpPackets>()),
        ("crate::ton::engine::validator::OnePerfTimerStat", std::mem::size_of::<crate::ton::engine::validator::OnePerfTimerStat>()),
        ("crate::ton::db::files::package::firstblock::FirstBlock", std::mem::size_of::<crate::ton::db::files::package::firstblock::FirstBlock>()),
        ("crate::ton::engine::dht::Dht", std::mem::size_of::<crate::ton::engine::dht::Dht>()),
        ("crate::ton::fees::Fees", std::mem::size_of::<crate::ton::fees::Fees>()),
        ("crate::ton::storage::db::storage::provider::db::contractaddress::ContractAddress", std::mem::size_of::<crate::ton::storage::db::storage::provider::db::contractaddress::ContractAddress>()),
        ("crate::ton::dht::config::Local", std::mem::size_of::<crate::ton::dht::config::Local>()),
        ("crate::ton::engine::validatortempkey::ValidatorTempKey", std::mem::size_of::<crate::ton::engine::validatortempkey::ValidatorTempKey>()),
        ("crate::ton::engine::validatoradnladdress::ValidatorAdnlAddress", std::mem::size_of::<crate::ton::engine::validatoradnladdress::ValidatorAdnlAddress>()),
        ("crate::ton::engine::adnl::Adnl", std::mem::size_of::<crate::ton::engine::adnl::Adnl>()),
        ("crate::ton::engine::validator::fullnodemaster::FullNodeMaster", std::mem::size_of::<crate::ton::engine::validator::fullnodemaster::FullNodeMaster>()),
        ("crate::ton::engine::liteserver::LiteServer", std::mem::size_of::<crate::ton::engine::liteserver::LiteServer>()),
        ("crate::ton::engine::controlprocess::ControlProcess", std::mem::size_of::<crate::ton::engine::controlprocess::ControlProcess>()),
        ("crate::ton::smc::LibraryQueryExt", std::mem::size_of::<crate::ton::smc::LibraryQueryExt>()),
        ("crate::ton::id::config::local::Local", std::mem::size_of::<crate::ton::id::config::local::Local>()),
        ("crate::ton::validator::config::Local", std::mem::size_of::<crate::ton::validator::config::Local>()),
        ("crate::ton::PublicKey", std::mem::size_of::<crate::ton::PublicKey>()),
        ("crate::ton::engine::validator::customoverlaynode::CustomOverlayNode", std::mem::size_of::<crate::ton::engine::validator::customoverlaynode::CustomOverlayNode>()),
        ("crate::ton::engine::validator::PerfTimerStatsByName", std::mem::size_of::<crate::ton::engine::validator::PerfTimerStatsByName>()),
        ("crate::ton::engine::validator::onesessionstat::OneSessionStat", std::mem::size_of::<crate::ton::engine::validator::onesessionstat::OneSessionStat>()),
        ("crate::ton::engine::validator::onestat::OneStat", std::mem::size_of::<crate::ton::engine::validator::onestat::OneStat>()),
        ("crate::ton::liteserver::config::Local", std::mem::size_of::<crate::ton::liteserver::config::Local>()),
        ("crate::ton::liteserver::desc_v2::Slice", std::mem::size_of::<crate::ton::liteserver::desc_v2::Slice>()),
        ("crate::ton::liteserver::desc::Desc", std::mem::size_of::<crate::ton::liteserver::desc::Desc>()),
        ("crate::ton::http::header::Header", std::mem::size_of::<crate::ton::http::header::Header>()),
        ("crate::ton::engine::validator::shard_overlay_stats::neighbour::Neighbour", std::mem::size_of::<crate::ton::engine::validator::shard_overlay_stats::neighbour::Neighbour>()),
        ("crate::ton::storage::daemon::fileinfo::FileInfo", std::mem::size_of::<crate::ton::storage::daemon::fileinfo::FileInfo>()),
        ("crate::ton::engine::validator::fullnodeslave::FullNodeSlave", std::mem::size_of::<crate::ton::engine::validator::fullnodeslave::FullNodeSlave>()),
        ("crate::ton::smc::libraryentry::LibraryEntry", std::mem::size_of::<crate::ton::smc::libraryentry::LibraryEntry>()),
        ("crate::ton::lite_server::libraryentry::LibraryEntry", std::mem::size_of::<crate::ton::lite_server::libraryentry::LibraryEntry>()),
        ("crate::ton::blocks::signature::Signature", std::mem::size_of::<crate::ton::blocks::signature::Signature>()),
        ("crate::ton::lite_server::accountdispatchqueueinfo::AccountDispatchQueueInfo", std::mem::size_of::<crate::ton::lite_server::accountdispatchqueueinfo::AccountDispatchQueueInfo>()),
        ("crate::ton::lite_server::signature::Signature", std::mem::size_of::<crate::ton::lite_server::signature::Signature>()),
        ("crate::ton::http::server::dnsentry::DnsEntry", std::mem::size_of::<crate::ton::http::server::dnsentry::DnsEntry>()),
        ("crate::ton::ton_node::blocksignature::BlockSignature", std::mem::size_of::<crate::ton::ton_node::blocksignature::BlockSignature>()),
        ("crate::ton::catchain::block::dep::Dep", std::mem::size_of::<crate::ton::catchain::block::dep::Dep>()),
        ("crate::ton::http::server::host::Host", std::mem::size_of::<crate::ton::http::server::host::Host>()),
        ("crate::ton::engine::controlinterface::ControlInterface", std::mem::size_of::<crate::ton::engine::controlinterface::ControlInterface>()),
        ("crate::ton::engine::adnl_proxy::port::Port", std::mem::size_of::<crate::ton::engine::adnl_proxy::port::Port>()),
        ("crate::ton::msg::dataencrypted::DataEncrypted", std::mem::size_of::<crate::ton::msg::dataencrypted::DataEncrypted>()),
        ("crate::ton::msg::datadecrypted::DataDecrypted", std::mem::size_of::<crate::ton::msg::datadecrypted::DataDecrypted>()),
        ("crate::ton::lite_server::nonfinal::validatorgroupinfo::ValidatorGroupInfo", std::mem::size_of::<crate::ton::lite_server::nonfinal::validatorgroupinfo::ValidatorGroupInfo>()),
        ("crate::ton::lite_server::blocks::transactionid::ShortTxId", std::mem::size_of::<crate::ton::lite_server::blocks::transactionid::ShortTxId>()),
        ("crate::ton::liteserver::descv2::DescV2", std::mem::size_of::<crate::ton::liteserver::descv2::DescV2>()),
        ("crate::ton::adnl::Address", std::mem::size_of::<crate::ton::adnl::Address>()),
        ("crate::ton::adnl::node::Node", std::mem::size_of::<crate::ton::adnl::node::Node>()),
        ("crate::ton::engine::validator::customoverlay::CustomOverlay", std::mem::size_of::<crate::ton::engine::validator::customoverlay::CustomOverlay>()),
        ("crate::ton::control::config::local::Local", std::mem::size_of::<crate::ton::control::config::local::Local>()),
        ("crate::ton::storage::daemon::peer::Peer", std::mem::size_of::<crate::ton::storage::daemon::peer::Peer>()),
        ("crate::ton::ton_node::blockidext::BlockIdExt", std::mem::size_of::<crate::ton::ton_node::blockidext::BlockIdExt>()),
        ("crate::ton::ton::BlockIdExt", std::mem::size_of::<crate::ton::ton::BlockIdExt>()),
        ("crate::ton::ton::blockidext::BlockIdExt", std::mem::size_of::<crate::ton::ton::blockidext::BlockIdExt>()),
        ("crate::ton::engine::validator::Validator", std::mem::size_of::<crate::ton::engine::validator::Validator>()),
        ("crate::ton::dns::Action", std::mem::size_of::<crate::ton::dns::Action>()),
        ("crate::ton::db::state::persistentstatedescriptionheader::PersistentStateDescriptionHeader", std::mem::size_of::<crate::ton::db::state::persistentstatedescriptionheader::PersistentStateDescriptionHeader>()),
        ("crate::ton::dns::entry::Entry", std::mem::size_of::<crate::ton::dns::entry::Entry>()),
        ("crate::ton::blocks::outmsgqueuesize::OutMsgQueueSize", std::mem::size_of::<crate::ton::blocks::outmsgqueuesize::OutMsgQueueSize>()),
        ("crate::ton::lite_server::outmsgqueuesize::OutMsgQueueSize", std::mem::size_of::<crate::ton::lite_server::outmsgqueuesize::OutMsgQueueSize>()),
        ("crate::ton::overlay::node::Node", std::mem::size_of::<crate::ton::overlay::node::Node>()),
        ("crate::ton::validator_session::round::Message", std::mem::size_of::<crate::ton::validator_session::round::Message>()),
        ("crate::ton::dht::node::Node", std::mem::size_of::<crate::ton::dht::node::Node>()),
        ("crate::ton::blocks::shardblocklink::ShardBlockLink", std::mem::size_of::<crate::ton::blocks::shardblocklink::ShardBlockLink>()),
        ("crate::ton::lite_server::shardblocklink::ShardBlockLink", std::mem::size_of::<crate::ton::lite_server::shardblocklink::ShardBlockLink>()),
        ("crate::ton::engine::Addr", std::mem::size_of::<crate::ton::engine::Addr>()),
        ("crate::ton::engine::validator::overlaystatsnode::OverlayStatsNode", std::mem::size_of::<crate::ton::engine::validator::overlaystatsnode::OverlayStatsNode>()),
        ("crate::ton::msg::message::Message", std::mem::size_of::<crate::ton::msg::message::Message>()),
        ("crate::ton::catchain::block::Block", std::mem::size_of::<crate::ton::catchain::block::Block>()),
        ("crate::ton::lite_server::transactionid::TransactionId", std::mem::size_of::<crate::ton::lite_server::transactionid::TransactionId>()),
        ("crate::ton::storage::daemon::contractinfo::ContractInfo", std::mem::size_of::<crate::ton::storage::daemon::contractinfo::ContractInfo>()),
        ("crate::ton::overlay::nodev2::NodeV2", std::mem::size_of::<crate::ton::overlay::nodev2::NodeV2>()),
        ("crate::ton::lite_server::nonfinal::candidateinfo::CandidateInfo", std::mem::size_of::<crate::ton::lite_server::nonfinal::candidateinfo::CandidateInfo>()),
        ("crate::ton::raw::message::Message", std::mem::size_of::<crate::ton::raw::message::Message>()),
        ("crate::ton::storage::daemon::torrent::Torrent", std::mem::size_of::<crate::ton::storage::daemon::torrent::Torrent>()),
        ("crate::ton::adnl::stats::localid::LocalId", std::mem::size_of::<crate::ton::adnl::stats::localid::LocalId>()),
        ("crate::ton::engine::validator::overlaystats::OverlayStats", std::mem::size_of::<crate::ton::engine::validator::overlaystats::OverlayStats>()),
        ("crate::ton::blocks::blocklinkback::BlockLinkBack", std::mem::size_of::<crate::ton::blocks::blocklinkback::BlockLinkBack>()),
        ("crate::ton::adnl::stats::peerpair::PeerPair", std::mem::size_of::<crate::ton::adnl::stats::peerpair::PeerPair>()),
        ("crate::ton::lite_server::BlockLink", std::mem::size_of::<crate::ton::lite_server::BlockLink>()),
        ("crate::ton::raw::transaction::Transaction", std::mem::size_of::<crate::ton::raw::transaction::Transaction>()),
        ("crate::ton::fullaccountstate::FullAccountState", std::mem::size_of::<crate::ton::fullaccountstate::FullAccountState>()),
        ("crate::ton::validator_session::statsproducer::StatsProducer", std::mem::size_of::<crate::ton::validator_session::statsproducer::StatsProducer>()),
    ];
    // for (name, size) in sizes {
    //     println!("{}: {} bytes", name, size);
    // }
    assert!(sizes.is_sorted_by(|a, b| a.1 <= b.1));
}
