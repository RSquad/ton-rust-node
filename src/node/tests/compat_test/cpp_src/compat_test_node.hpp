#pragma once

#include "adnl/adnl.h"
#include "adnl/adnl-network-manager.h"
#include "adnl/adnl-address-list.h"
#include "overlay/overlays.h"
#include "overlay/overlay-id.hpp"
#include "rldp/rldp.h"
#include "rldp2/rldp.h"
#include "quic-sender.h"
#include "td/fec/fec.h"
#include "keys/keys.hpp"
#include "keyring/keyring.h"
#include "td/actor/actor.h"
#include "td/utils/JsonBuilder.h"
#include "td/utils/base64.h"
#include "auto/tl/ton_api.h"
#include "auto/tl/ton_api_json.h"
#include "tl-utils/tl-utils.hpp"

#include <iostream>
#include <memory>
#include <map>
#include <functional>
#include <set>

namespace compat_test {

// Received broadcast record
struct ReceivedBroadcast {
    ton::PublicKeyHash source;
    ton::overlay::OverlayIdShort overlay_id;
    std::vector<td::uint8> data;
    td::int32 timestamp;
    bool was_accepted;
};

// Received message record (point-to-point overlay messages)
struct ReceivedMessage {
    ton::adnl::AdnlNodeIdShort source;
    ton::overlay::OverlayIdShort overlay_id;
    std::vector<td::uint8> data;
    td::int32 timestamp;
};

// Per-overlay state
struct OverlayState {
    ton::overlay::OverlayIdFull id_full;
    ton::overlay::OverlayIdShort id_short;
    std::string type;  // "public", "private", "semiprivate"
    std::string query_handler_mode = "echo";  // "echo", "capabilities"
    std::string broadcast_validator_mode = "accept_all";  // "accept_all", "reject_all"
    std::vector<ReceivedBroadcast> received_broadcasts;
    std::vector<ReceivedMessage> received_messages;
    std::vector<std::pair<std::string, size_t>> received_queries;  // (from_hex, data_size)
};

// Overlay callback implementation for testing
class TestOverlayCallback : public ton::overlay::Overlays::Callback {
public:
    using BroadcastHandler = std::function<void(ton::PublicKeyHash, td::BufferSlice)>;
    using QueryHandler = std::function<void(ton::adnl::AdnlNodeIdShort, td::BufferSlice, td::Promise<td::BufferSlice>)>;
    using CheckBroadcastHandler = std::function<void(ton::PublicKeyHash, td::BufferSlice, td::Promise<td::Unit>)>;
    using MessageHandler = std::function<void(ton::adnl::AdnlNodeIdShort, td::BufferSlice)>;

    TestOverlayCallback(
        ton::overlay::OverlayIdShort overlay_id,
        BroadcastHandler on_broadcast,
        QueryHandler on_query,
        CheckBroadcastHandler check_broadcast,
        MessageHandler on_message = nullptr
    ) : overlay_id_(overlay_id)
      , on_broadcast_(std::move(on_broadcast))
      , on_query_(std::move(on_query))
      , check_broadcast_(std::move(check_broadcast))
      , on_message_(std::move(on_message)) {}

    void receive_message(ton::adnl::AdnlNodeIdShort src,
                        ton::overlay::OverlayIdShort overlay_id,
                        td::BufferSlice data) override {
        LOG(INFO) << "MSG_RECEIVED overlay=" << overlay_id.bits256_value().to_hex()
                  << " src=" << src.bits256_value().to_hex()
                  << " size=" << data.size();
        if (on_message_) {
            on_message_(src, std::move(data));
        }
    }

    void receive_query(ton::adnl::AdnlNodeIdShort src,
                      ton::overlay::OverlayIdShort overlay_id,
                      td::BufferSlice data,
                      td::Promise<td::BufferSlice> promise) override {
        LOG(INFO) << "QUERY_RECEIVED overlay=" << overlay_id.bits256_value().to_hex()
                  << " src=" << src.bits256_value().to_hex()
                  << " size=" << data.size();
        if (on_query_) {
            on_query_(src, std::move(data), std::move(promise));
        } else {
            // Default: echo back
            promise.set_value(std::move(data));
        }
    }

    void receive_broadcast(ton::PublicKeyHash src,
                          ton::overlay::OverlayIdShort overlay_id,
                          td::BufferSlice data) override {
        LOG(INFO) << "BROADCAST_DELIVERED overlay=" << overlay_id.bits256_value().to_hex()
                  << " src=" << src.bits256_value().to_hex()
                  << " size=" << data.size();
        if (on_broadcast_) {
            on_broadcast_(src, std::move(data));
        }
    }

    void check_broadcast(ton::PublicKeyHash src,
                        ton::overlay::OverlayIdShort overlay_id,
                        td::BufferSlice data,
                        td::Promise<td::Unit> promise) override {
        LOG(INFO) << "CHECK_BROADCAST overlay=" << overlay_id.bits256_value().to_hex()
                  << " src=" << src.bits256_value().to_hex()
                  << " size=" << data.size();
        if (check_broadcast_) {
            check_broadcast_(src, std::move(data), std::move(promise));
        } else {
            promise.set_value(td::Unit());
        }
    }

private:
    ton::overlay::OverlayIdShort overlay_id_;
    BroadcastHandler on_broadcast_;
    QueryHandler on_query_;
    CheckBroadcastHandler check_broadcast_;
    MessageHandler on_message_;
};

// Main test node actor
class CompatTestNode : public td::actor::Actor {
public:
    struct Config {
        td::uint16 udp_port = 14000;
        std::string db_path = "/tmp/compat_test_node";
    };

    explicit CompatTestNode(Config config);

    void start_up() override;
    void tear_down() override;
    void alarm() override;

private:
    Config config_;

    // ADNL components
    td::actor::ActorOwn<ton::adnl::AdnlNetworkManager> network_manager_;
    td::actor::ActorOwn<ton::adnl::Adnl> adnl_;
    td::actor::ActorOwn<ton::keyring::Keyring> keyring_;
    td::actor::ActorOwn<ton::overlay::Overlays> overlays_;
    td::actor::ActorOwn<ton::rldp::Rldp> rldp_;
    td::actor::ActorOwn<ton::rldp2::Rldp> rldp2_;
    td::actor::ActorOwn<ton::quic::QuicSender> quic_;

    // Local identity
    ton::PrivateKey local_privkey_;
    ton::PublicKey local_pubkey_;
    ton::adnl::AdnlNodeIdShort local_id_short_;

    // Active overlays
    std::map<ton::overlay::OverlayIdShort, OverlayState> overlay_states_;

    // Control interface
    void process_stdin();
    void handle_command(std::string cmd_line);

    // Command handlers
    void cmd_get_info(td::JsonObject &obj);
    void cmd_compute_overlay_id(td::JsonObject &obj);
    void cmd_add_peer(td::JsonObject &obj);
    void cmd_create_overlay(td::JsonObject &obj);
    void cmd_delete_overlay(td::JsonObject &obj);
    void cmd_get_overlay_node_info(td::JsonObject &obj);
    void cmd_send_broadcast(td::JsonObject &obj);
    void cmd_send_query(td::JsonObject &obj);
    void cmd_send_rldp_query(td::JsonObject &obj);
    void cmd_set_query_handler(td::JsonObject &obj);
    void cmd_set_broadcast_validator(td::JsonObject &obj);
    void cmd_get_received_broadcasts(td::JsonObject &obj);
    void cmd_clear_received_broadcasts(td::JsonObject &obj);
    void cmd_send_message(td::JsonObject &obj);
    void cmd_get_received_messages(td::JsonObject &obj);
    void cmd_clear_received_messages(td::JsonObject &obj);
    void cmd_compress_boc(td::JsonObject &obj);
    void cmd_decompress_boc(td::JsonObject &obj);
    void cmd_compute_candidate_id_to_sign(td::JsonObject &obj);
    void cmd_enable_quic(td::JsonObject &obj);
    void cmd_send_quic_message(td::JsonObject &obj);
    void cmd_send_quic_query(td::JsonObject &obj);
    void cmd_raptorq_encode(td::JsonObject &obj);
    void cmd_raptorq_decode(td::JsonObject &obj);

    // Helpers
    std::string get_string(td::JsonObject &obj, const std::string &key);
    bool get_bool(td::JsonObject &obj, const std::string &key, bool def = false);
    td::int64 get_int(td::JsonObject &obj, const std::string &key, td::int64 def = 0);

    std::unique_ptr<TestOverlayCallback> make_overlay_callback(ton::overlay::OverlayIdShort overlay_id);

    void on_broadcast_received(ton::overlay::OverlayIdShort overlay_id,
                               ton::PublicKeyHash source, td::BufferSlice data);
    void on_message_received(ton::overlay::OverlayIdShort overlay_id,
                             ton::adnl::AdnlNodeIdShort source, td::BufferSlice data);
    void on_query_received(ton::overlay::OverlayIdShort overlay_id,
                          ton::adnl::AdnlNodeIdShort src, td::BufferSlice data,
                          td::Promise<td::BufferSlice> promise);
    void on_check_broadcast(ton::overlay::OverlayIdShort overlay_id,
                           ton::PublicKeyHash source, td::BufferSlice data,
                           td::Promise<td::Unit> promise);

    void respond(const std::string &json);
    void respond_ok();
    void respond_ok(const std::string &extra_fields);
    void respond_error(const std::string &msg);
};

} // namespace compat_test
