/*
 * Cross-Implementation Compatibility Test Node
 *
 * A minimal ADNL/overlay node for testing compatibility between rust and cpp implementations.
 *
 * Controlled via stdin/stdout by end of line terminated JSON:
 *
 *   {"cmd": "ping"}
 *   {"cmd": "get_info"}
 *   {"cmd": "compute_overlay_id", "name": "BASE64_BYTES"}
 *   {"cmd": "add_peer", "pubkey": "BASE64_TL_PUBKEY", "ip": "127.0.0.1", "port": 14001, "quic_port": 0}
 *   {"cmd": "create_overlay", "type": "public|private|semiprivate",
 *           "overlay_name": "BASE64_TL_BYTES",
 *           "peers": ["ADNL_ID_HEX", ...],
 *           "root_pub_keys": ["HEX", ...], "certificate": "BASE64_TL", "max_slaves": 5}
 *   {"cmd": "delete_overlay", "overlay_id": "HEX"}
 *   {"cmd": "get_overlay_node_info", "overlay_id": "HEX"}
 *   {"cmd": "send_broadcast", "overlay_id": "HEX", "data": "BASE64", "use_fec": false}
 *   {"cmd": "send_query", "overlay_id": "HEX", "peer_adnl_id": "HEX",
 *           "data": "BASE64", "timeout_ms": 5000}
 *   {"cmd": "send_rldp_query", "overlay_id": "HEX", "peer_adnl_id": "HEX",
 *           "data": "BASE64", "max_answer_size": 1048576, "v2": false}
 *   {"cmd": "set_query_handler", "overlay_id": "HEX", "mode": "echo|capabilities|reject"}
 *   {"cmd": "set_broadcast_validator", "overlay_id": "HEX", "mode": "accept_all|reject_all"}
 *   {"cmd": "get_received_broadcasts", "overlay_id": "HEX"}
 *   {"cmd": "clear_received_broadcasts", "overlay_id": "HEX"}
 *   {"cmd": "compute_candidate_id_to_sign", "slot": 1, "hash": "HEX_64"}
 *   {"cmd": "compress_boc", "data": "BASE64_STANDARD_BOC", "algorithm": "baseline|improved"}
 *   {"cmd": "decompress_boc", "data": "BASE64_COMPRESSED", "max_size": 10485760}
 *   {"cmd": "shutdown"}
 */

#include "compat_test_node.hpp"

#include "td/utils/port/Stat.h"
#include "td/utils/port/path.h"
#include "td/utils/Random.h"
#include "crypto/Ed25519.h"
#include "vm/boc.h"
#include "vm/boc-compression.h"

#include <sstream>
#include <unistd.h>

namespace compat_test {

// ---------- Helpers ----------

CompatTestNode::CompatTestNode(Config config) : config_(std::move(config)) {}

std::string CompatTestNode::get_string(td::JsonObject &obj, const std::string &key) {
    for (auto &kv : obj.field_values_) {
        if (kv.first == key && kv.second.type() == td::JsonValue::Type::String) {
            return kv.second.get_string().str();
        }
    }
    return "";
}

bool CompatTestNode::get_bool(td::JsonObject &obj, const std::string &key, bool def) {
    for (auto &kv : obj.field_values_) {
        if (kv.first == key && kv.second.type() == td::JsonValue::Type::Boolean) {
            return kv.second.get_boolean();
        }
    }
    return def;
}

td::int64 CompatTestNode::get_int(td::JsonObject &obj, const std::string &key, td::int64 def) {
    for (auto &kv : obj.field_values_) {
        if (kv.first == key && kv.second.type() == td::JsonValue::Type::Number) {
            return td::to_integer<td::int64>(kv.second.get_number());
        }
    }
    return def;
}

void CompatTestNode::respond(const std::string &json) {
    std::cout << json << std::endl;
    std::cout.flush();
}

void CompatTestNode::respond_ok() {
    respond("{\"result\": \"ok\"}");
}

void CompatTestNode::respond_ok(const std::string &extra_fields) {
    respond("{\"result\": {" + extra_fields + "}}");
}

void CompatTestNode::respond_error(const std::string &msg) {
    // Escape quotes in msg
    std::string escaped;
    for (char c : msg) {
        if (c == '"') escaped += "\\\"";
        else if (c == '\\') escaped += "\\\\";
        else if (c == '\n') escaped += "\\n";
        else escaped += c;
    }
    respond("{\"error\": \"" + escaped + "\"}");
}

// ---------- Startup ----------

void CompatTestNode::start_up() {
    LOG(INFO) << "Starting compat test node on UDP port " << config_.udp_port;

    // Create database directory
    td::mkdir(config_.db_path).ignore();

    // Generate local key
    local_privkey_ = ton::PrivateKey{ton::privkeys::Ed25519::random()};
    local_pubkey_ = local_privkey_.compute_public_key();
    local_id_short_ = ton::adnl::AdnlNodeIdShort{local_pubkey_.compute_short_id()};

    LOG(INFO) << "Local ADNL ID: " << local_id_short_.bits256_value().to_hex();

    // Create keyring
    keyring_ = ton::keyring::Keyring::create(config_.db_path + "/keyring");

    // Add local key to keyring - generate a new key for keyring since we can't clone
    auto keyring_privkey = ton::PrivateKey{ton::privkeys::Ed25519::random()};
    // Actually, we need to use the same key. Let's re-generate with the same approach
    // and store export/import to share between local and keyring
    auto key_slice = local_privkey_.export_as_slice();
    auto key_import = ton::PrivateKey::import(key_slice.as_slice());
    CHECK(key_import.is_ok());
    td::actor::send_closure(keyring_, &ton::keyring::Keyring::add_key,
                           key_import.move_as_ok(), true,
                           td::PromiseCreator::lambda([](td::Result<td::Unit>) {}));

    // Create network manager with real UDP
    network_manager_ = ton::adnl::AdnlNetworkManager::create(config_.udp_port);

    // Register self address on network manager
    td::IPAddress self_addr;
    self_addr.init_ipv4_port("127.0.0.1", config_.udp_port).ensure();

    ton::adnl::AdnlCategoryMask cat_mask;
    cat_mask[0] = true;
    td::actor::send_closure(network_manager_, &ton::adnl::AdnlNetworkManager::add_self_addr,
                           self_addr, std::move(cat_mask), 0);

    // Create ADNL instance
    adnl_ = ton::adnl::Adnl::create(config_.db_path, keyring_.get());

    // Register network manager with ADNL
    td::actor::send_closure(adnl_, &ton::adnl::Adnl::register_network_manager,
                           network_manager_.get());

    // Build proper address list for our identity
    ton::adnl::AdnlAddressList addr_list;
    addr_list.add_udp_adnl_address(self_addr).ensure();
    addr_list.set_version(static_cast<td::int32>(td::Clocks::system()));
    addr_list.set_reinit_date(ton::adnl::Adnl::adnl_start_time());

    // Add local ID to ADNL with proper address list
    td::actor::send_closure(adnl_, &ton::adnl::Adnl::add_id,
                           ton::adnl::AdnlNodeIdFull{local_pubkey_},
                           std::move(addr_list), static_cast<td::uint8>(0));

    // Create RLDP v1 and v2
    rldp_ = ton::rldp::Rldp::create(adnl_.get());
    rldp2_ = ton::rldp2::Rldp::create(adnl_.get());
    td::actor::send_closure(rldp_, &ton::rldp::Rldp::add_id, local_id_short_);
    td::actor::send_closure(rldp2_, &ton::rldp2::Rldp::add_id, local_id_short_);

    // Create overlay manager (without DHT for direct peering)
    overlays_ = ton::overlay::Overlays::create(config_.db_path, keyring_.get(),
                                                adnl_.get(), td::actor::ActorId<ton::dht::Dht>{});

    // Setup stdin polling
    alarm_timestamp() = td::Timestamp::in(0.1);

    // Output ready message
    auto pubkey_tl = local_pubkey_.tl();
    auto pubkey_serialized = ton::serialize_tl_object(pubkey_tl, true);
    auto pubkey_b64 = td::base64_encode(pubkey_serialized.as_slice());
    std::ostringstream oss;
    oss << "{\"status\": \"ready\""
        << ", \"adnl_id\": \"" << local_id_short_.bits256_value().to_hex() << "\""
        << ", \"pubkey\": \"" << pubkey_b64 << "\""
        << ", \"udp_port\": " << config_.udp_port
        << "}";
    respond(oss.str());
}

void CompatTestNode::tear_down() {
    LOG(INFO) << "Shutting down compat test node";
}

void CompatTestNode::alarm() {
    process_stdin();
    alarm_timestamp() = td::Timestamp::in(0.1);
}

// ---------- Control interface ----------

void CompatTestNode::process_stdin() {
    fd_set readfds;
    FD_ZERO(&readfds);
    FD_SET(STDIN_FILENO, &readfds);

    struct timeval tv;
    tv.tv_sec = 0;
    tv.tv_usec = 0;

    if (select(STDIN_FILENO + 1, &readfds, nullptr, nullptr, &tv) > 0) {
        std::string line;
        if (std::getline(std::cin, line)) {
            handle_command(line);
        } else {
            // stdin closed
            LOG(INFO) << "stdin closed, shutting down";
            stop();
        }
    }
}

void CompatTestNode::handle_command(std::string cmd_line) {
    if (cmd_line.empty()) return;

    if (cmd_line.size() <= 250) {
        LOG(INFO) << "CMD: " << cmd_line;
    } else {
        LOG(INFO) << "CMD: " << cmd_line.substr(0, 250) << "... (" << cmd_line.size() << " bytes)";
    }

    auto json_res = td::json_decode(cmd_line);
    if (json_res.is_error()) {
        respond_error("Invalid JSON: " + json_res.error().message().str());
        return;
    }

    auto &json = json_res.ok_ref();
    if (json.type() != td::JsonValue::Type::Object) {
        respond_error("Expected JSON object");
        return;
    }

    auto &obj = json.get_object();
    auto cmd = get_string(obj, "cmd");

    if (cmd.empty()) {
        respond_error("Missing 'cmd' field");
        return;
    }

    if (cmd == "ping") {
        respond("{\"result\": \"pong\"}");
    } else if (cmd == "get_info") {
        cmd_get_info(obj);
    } else if (cmd == "compute_overlay_id") {
        cmd_compute_overlay_id(obj);
    } else if (cmd == "add_peer") {
        cmd_add_peer(obj);
    } else if (cmd == "create_overlay") {
        cmd_create_overlay(obj);
    } else if (cmd == "delete_overlay") {
        cmd_delete_overlay(obj);
    } else if (cmd == "get_overlay_node_info") {
        cmd_get_overlay_node_info(obj);
    } else if (cmd == "send_broadcast") {
        cmd_send_broadcast(obj);
    } else if (cmd == "send_query") {
        cmd_send_query(obj);
    } else if (cmd == "send_rldp_query") {
        cmd_send_rldp_query(obj);
    } else if (cmd == "set_query_handler") {
        cmd_set_query_handler(obj);
    } else if (cmd == "set_broadcast_validator") {
        cmd_set_broadcast_validator(obj);
    } else if (cmd == "get_received_broadcasts") {
        cmd_get_received_broadcasts(obj);
    } else if (cmd == "clear_received_broadcasts") {
        cmd_clear_received_broadcasts(obj);
    } else if (cmd == "send_message") {
        cmd_send_message(obj);
    } else if (cmd == "get_received_messages") {
        cmd_get_received_messages(obj);
    } else if (cmd == "clear_received_messages") {
        cmd_clear_received_messages(obj);
    } else if (cmd == "compute_candidate_id_to_sign") {
        cmd_compute_candidate_id_to_sign(obj);
    } else if (cmd == "compress_boc") {
        cmd_compress_boc(obj);
    } else if (cmd == "decompress_boc") {
        cmd_decompress_boc(obj);
    } else if (cmd == "enable_quic") {
        cmd_enable_quic(obj);
    } else if (cmd == "send_quic_message") {
        cmd_send_quic_message(obj);
    } else if (cmd == "send_quic_query") {
        cmd_send_quic_query(obj);
    } else if (cmd == "raptorq_encode") {
        cmd_raptorq_encode(obj);
    } else if (cmd == "raptorq_decode") {
        cmd_raptorq_decode(obj);
    } else if (cmd == "shutdown") {
        respond("{\"result\": \"shutting_down\"}");
        std::_Exit(0);  // Force immediate exit
    } else {
        respond_error("Unknown command: " + cmd);
    }
}

// ---------- Command implementations ----------

void CompatTestNode::cmd_get_info(td::JsonObject &obj) {
    auto pubkey_tl = local_pubkey_.tl();
    auto pubkey_serialized = ton::serialize_tl_object(pubkey_tl, true);
    auto pubkey_b64 = td::base64_encode(pubkey_serialized.as_slice());
    std::ostringstream oss;
    oss << "{\"result\": {"
        << "\"adnl_id\": \"" << local_id_short_.bits256_value().to_hex() << "\""
        << ", \"pubkey\": \"" << pubkey_b64 << "\""
        << ", \"udp_port\": " << config_.udp_port
        << "}}";
    respond(oss.str());
}

void CompatTestNode::cmd_compute_overlay_id(td::JsonObject &obj) {
    auto name_b64 = get_string(obj, "name");
    if (name_b64.empty()) {
        respond_error("Missing 'name' field (base64)");
        return;
    }
    auto name_res = td::base64_decode(name_b64);
    if (name_res.is_error()) {
        respond_error("Invalid base64 name");
        return;
    }
    auto name = name_res.move_as_ok();

    ton::overlay::OverlayIdFull full_id{td::BufferSlice(name)};
    auto short_id = full_id.compute_short_id();

    std::ostringstream oss;
    oss << "{\"result\": {"
        << "\"overlay_id\": \"" << short_id.bits256_value().to_hex() << "\""
        << "}}";
    respond(oss.str());
}

void CompatTestNode::cmd_add_peer(td::JsonObject &obj) {
    auto pubkey_b64 = get_string(obj, "pubkey");
    auto ip = get_string(obj, "ip");
    auto port = static_cast<td::uint16>(get_int(obj, "port"));

    if (pubkey_b64.empty() || ip.empty() || port == 0) {
        respond_error("Missing 'pubkey', 'ip', or 'port'");
        return;
    }

    auto pubkey_res = td::base64_decode(pubkey_b64);
    if (pubkey_res.is_error()) {
        respond_error("Invalid base64 pubkey");
        return;
    }
    auto pk_res = ton::PublicKey::import(pubkey_res.ok());
    if (pk_res.is_error()) {
        respond_error("Invalid pubkey format: " + pk_res.error().message().str());
        return;
    }
    auto pk = pk_res.move_as_ok();

    td::IPAddress addr;
    auto addr_res = addr.init_ipv4_port(ip, port);
    if (addr_res.is_error()) {
        respond_error("Invalid address: " + addr_res.message().str());
        return;
    }

    // Build address list for the peer
    ton::adnl::AdnlAddressList peer_addr_list;
    peer_addr_list.add_udp_adnl_address(addr).ensure();
    auto quic_port = static_cast<td::uint16>(get_int(obj, "quic_port"));
    if (quic_port != 0) {
        td::IPAddress quic_addr;
        quic_addr.init_ipv4_port(ip, quic_port).ensure();
        peer_addr_list.add_quic_addr(quic_addr).ensure();
    }
    peer_addr_list.set_version(static_cast<td::int32>(td::Clocks::system()));
    peer_addr_list.set_reinit_date(ton::adnl::Adnl::adnl_start_time());

    auto peer_id = ton::adnl::AdnlNodeIdFull{pk};
    auto peer_short = peer_id.compute_short_id();

    td::actor::send_closure(adnl_, &ton::adnl::Adnl::add_peer,
                           local_id_short_, peer_id, std::move(peer_addr_list));

    respond_ok("\"peer_id\": \"" + peer_short.bits256_value().to_hex() + "\"");
}

void CompatTestNode::cmd_create_overlay(td::JsonObject &obj) {
    auto type = get_string(obj, "type");
    auto overlay_name_b64 = get_string(obj, "overlay_name");

    if (type.empty()) {
        respond_error("Missing 'type' (public|private|semiprivate)");
        return;
    }
    if (overlay_name_b64.empty()) {
        respond_error("Missing 'overlay_name' (base64 TL bytes)");
        return;
    }

    auto name_res = td::base64_decode(overlay_name_b64);
    if (name_res.is_error()) {
        respond_error("Invalid base64 overlay_name");
        return;
    }
    auto name = name_res.move_as_ok();

    ton::overlay::OverlayIdFull id_full{td::BufferSlice(name)};
    auto id_short = id_full.compute_short_id();

    LOG(INFO) << "Creating " << type << " overlay: " << id_short.bits256_value().to_hex();

    auto callback = make_overlay_callback(id_short);

    // Use permissive rules: allow broadcasts up to 32MB with AllowFec flag.
    // NOTE: We do NOT set CertificateFlags::Trusted here because that would skip
    // the check_broadcast callback entirely. Without Trusted, all broadcasts go
    // through the 2-phase validation (check_broadcast callback).
    td::uint32 max_bcast_size = 32 << 20;  // 32 MB
    td::uint32 privacy_flags = ton::overlay::CertificateFlags::AllowFec;

    if (type == "public") {
        ton::overlay::OverlayOptions opts;
        opts.announce_self_ = false;  // No DHT

        ton::overlay::OverlayPrivacyRules rules{max_bcast_size, privacy_flags, {}};
        td::actor::send_closure(overlays_, &ton::overlay::Overlays::create_public_overlay_ex,
                               local_id_short_,
                               id_full.clone(),
                               std::move(callback),
                               std::move(rules),
                               "compat_test",
                               opts);
    } else if (type == "private") {
        // Parse peer list
        std::vector<ton::adnl::AdnlNodeIdShort> peers;
        for (auto &kv : obj.field_values_) {
            if (kv.first == "peers" && kv.second.type() == td::JsonValue::Type::Array) {
                for (auto &p : kv.second.get_array()) {
                    if (p.type() == td::JsonValue::Type::String) {
                        td::Bits256 bits;
                        auto hex = p.get_string().str();
                        if (bits.from_hex(hex) == 256) {
                            peers.push_back(ton::adnl::AdnlNodeIdShort{bits});
                        }
                    }
                }
                break;
            }
        }

        ton::overlay::OverlayPrivacyRules rules{max_bcast_size, privacy_flags, {}};
        ton::overlay::OverlayOptions opts;
        opts.announce_self_ = false;

        auto enable_twostep = get_bool(obj, "enable_twostep", false);
        if (enable_twostep) {
            opts.twostep_broadcast_sender_ = rldp2_.get();
            opts.send_twostep_broadcast_ = true;
            LOG(INFO) << "TwostepFec enabled for private overlay";
        }

        td::actor::send_closure(overlays_, &ton::overlay::Overlays::create_private_overlay_ex,
                               local_id_short_,
                               id_full.clone(),
                               std::move(peers),
                               std::move(callback),
                               std::move(rules),
                               "compat_test",
                               std::move(opts));
    } else if (type == "semiprivate") {
        // Parse peer list
        std::vector<ton::adnl::AdnlNodeIdShort> peers;
        for (auto &kv : obj.field_values_) {
            if (kv.first == "peers" && kv.second.type() == td::JsonValue::Type::Array) {
                for (auto &p : kv.second.get_array()) {
                    if (p.type() == td::JsonValue::Type::String) {
                        td::Bits256 bits;
                        auto hex = p.get_string().str();
                        if (bits.from_hex(hex) == 256) {
                            peers.push_back(ton::adnl::AdnlNodeIdShort{bits});
                        }
                    }
                }
                break;
            }
        }

        // Parse root public key hashes
        std::vector<ton::PublicKeyHash> root_keys;
        for (auto &kv : obj.field_values_) {
            if (kv.first == "root_pub_keys" && kv.second.type() == td::JsonValue::Type::Array) {
                for (auto &r : kv.second.get_array()) {
                    if (r.type() == td::JsonValue::Type::String) {
                        td::Bits256 bits;
                        auto hex = r.get_string().str();
                        if (bits.from_hex(hex) == 256) {
                            root_keys.push_back(ton::PublicKeyHash{bits});
                        }
                    }
                }
                break;
            }
        }

        // Parse certificate
        ton::overlay::OverlayMemberCertificate cert;
        auto cert_b64 = get_string(obj, "certificate");
        if (!cert_b64.empty()) {
            auto cert_res = td::base64_decode(cert_b64);
            if (cert_res.is_ok()) {
                auto cert_data = cert_res.move_as_ok();
                auto tl_res = ton::fetch_tl_object<ton::ton_api::overlay_MemberCertificate>(
                    td::BufferSlice(cert_data), true);
                if (tl_res.is_ok()) {
                    cert = ton::overlay::OverlayMemberCertificate(tl_res.ok().get());
                }
            }
        }

        auto max_slaves = static_cast<td::int32>(get_int(obj, "max_slaves", 5));

        ton::overlay::OverlayOptions opts;
        opts.announce_self_ = false;
        opts.max_slaves_in_semiprivate_overlay_ = max_slaves;

        ton::overlay::OverlayPrivacyRules rules{max_bcast_size, privacy_flags, {}};
        td::actor::send_closure(overlays_, &ton::overlay::Overlays::create_semiprivate_overlay,
                               local_id_short_,
                               id_full.clone(),
                               std::move(peers),
                               std::move(root_keys),
                               std::move(cert),
                               std::move(callback),
                               std::move(rules),
                               "compat_test",
                               opts);
    } else {
        respond_error("Unknown overlay type: " + type);
        return;
    }

    // Store overlay state
    OverlayState state;
    state.id_full = std::move(id_full);
    state.id_short = id_short;
    state.type = type;
    overlay_states_[id_short] = std::move(state);

    respond_ok("\"overlay_id\": \"" + id_short.bits256_value().to_hex() + "\"");
}

void CompatTestNode::cmd_delete_overlay(td::JsonObject &obj) {
    auto overlay_hex = get_string(obj, "overlay_id");
    td::Bits256 bits;
    if (bits.from_hex(overlay_hex) != 256) {
        respond_error("Invalid overlay_id hex");
        return;
    }
    auto id_short = ton::overlay::OverlayIdShort{bits};

    td::actor::send_closure(overlays_, &ton::overlay::Overlays::delete_overlay,
                           local_id_short_, id_short);
    overlay_states_.erase(id_short);
    respond_ok();
}

void CompatTestNode::cmd_get_overlay_node_info(td::JsonObject &obj) {
    auto overlay_hex = get_string(obj, "overlay_id");
    td::Bits256 bits;
    if (bits.from_hex(overlay_hex) != 256) {
        respond_error("Invalid overlay_id hex");
        return;
    }
    auto overlay_id = ton::overlay::OverlayIdShort{bits};

    auto it = overlay_states_.find(overlay_id);
    if (it == overlay_states_.end()) {
        respond_error("Overlay not found");
        return;
    }

    // Build OverlayNode, sign it via keyring, serialize as TL
    auto node = ton::overlay::OverlayNode{local_id_short_, overlay_id, 0};
    auto to_sign = node.to_sign();

    td::actor::send_closure(
        keyring_, &ton::keyring::Keyring::sign_add_get_public_key,
        local_id_short_.pubkey_hash(), std::move(to_sign),
        [SelfId = actor_id(this), overlay_id](
            td::Result<std::pair<td::BufferSlice, ton::PublicKey>> R) {
            if (R.is_error()) {
                td::actor::send_closure(SelfId, &CompatTestNode::respond_error,
                                       "Failed to sign: " + R.error().message().str());
                return;
            }
            auto V = R.move_as_ok();
            auto node = ton::overlay::OverlayNode{
                ton::adnl::AdnlNodeIdFull{V.second}, overlay_id, 0,
                static_cast<td::int32>(td::Clocks::system()), V.first.as_slice()};
            auto tl = node.tl();
            auto serialized = ton::serialize_tl_object(tl, true);
            auto b64 = td::base64_encode(serialized.as_slice());
            std::ostringstream oss;
            oss << "{\"result\": {\"node_tl\": \"" << b64 << "\"}}";
            td::actor::send_closure(SelfId, &CompatTestNode::respond, oss.str());
        });
}

void CompatTestNode::cmd_send_broadcast(td::JsonObject &obj) {
    auto overlay_hex = get_string(obj, "overlay_id");
    auto data_b64 = get_string(obj, "data");
    auto use_fec = get_bool(obj, "use_fec", false);

    td::Bits256 bits;
    if (bits.from_hex(overlay_hex) != 256) {
        respond_error("Invalid overlay_id hex");
        return;
    }
    if (data_b64.empty()) {
        respond_error("Missing 'data'");
        return;
    }
    auto data_res = td::base64_decode(data_b64);
    if (data_res.is_error()) {
        respond_error("Invalid base64 data");
        return;
    }

    auto overlay_id = ton::overlay::OverlayIdShort{bits};
    auto data = td::BufferSlice(data_res.move_as_ok());

    LOG(INFO) << "Sending " << (use_fec ? "FEC " : "") << "broadcast to overlay "
              << overlay_id.bits256_value().to_hex() << " size=" << data.size();

    if (use_fec) {
        td::actor::send_closure(overlays_, &ton::overlay::Overlays::send_broadcast_fec_ex,
                               local_id_short_, overlay_id, local_id_short_.pubkey_hash(),
                               0, std::move(data));
    } else {
        td::actor::send_closure(overlays_, &ton::overlay::Overlays::send_broadcast_ex,
                               local_id_short_, overlay_id, local_id_short_.pubkey_hash(),
                               0, std::move(data));
    }
    respond_ok();
}

void CompatTestNode::cmd_send_query(td::JsonObject &obj) {
    auto overlay_hex = get_string(obj, "overlay_id");
    auto peer_hex = get_string(obj, "peer_adnl_id");
    auto data_b64 = get_string(obj, "data");
    auto timeout_ms = get_int(obj, "timeout_ms", 5000);

    td::Bits256 overlay_bits, peer_bits;
    if (overlay_bits.from_hex(overlay_hex) != 256) {
        respond_error("Invalid overlay_id hex");
        return;
    }
    if (peer_bits.from_hex(peer_hex) != 256) {
        respond_error("Invalid peer_adnl_id hex");
        return;
    }

    auto data_res = td::base64_decode(data_b64);
    if (data_res.is_error()) {
        respond_error("Invalid base64 data");
        return;
    }

    auto overlay_id = ton::overlay::OverlayIdShort{overlay_bits};
    auto peer_id = ton::adnl::AdnlNodeIdShort{peer_bits};
    auto data = td::BufferSlice(data_res.move_as_ok());
    auto timeout = td::Timestamp::in(timeout_ms / 1000.0);

    td::actor::send_closure(
        overlays_, &ton::overlay::Overlays::send_query,
        peer_id, local_id_short_, overlay_id, "compat_test_query",
        td::PromiseCreator::lambda(
            [SelfId = actor_id(this)](td::Result<td::BufferSlice> R) {
                if (R.is_error()) {
                    td::actor::send_closure(SelfId, &CompatTestNode::respond_error,
                                           "Query failed: " + R.error().message().str());
                    return;
                }
                auto answer = R.move_as_ok();
                auto b64 = td::base64_encode(answer.as_slice());
                std::ostringstream oss;
                oss << "{\"result\": {\"answer\": \"" << b64 << "\"}}";
                td::actor::send_closure(SelfId, &CompatTestNode::respond, oss.str());
            }),
        timeout, std::move(data));
}

void CompatTestNode::cmd_send_rldp_query(td::JsonObject &obj) {
    auto overlay_hex = get_string(obj, "overlay_id");
    auto peer_hex = get_string(obj, "peer_adnl_id");
    auto data_b64 = get_string(obj, "data");
    auto max_answer_size = static_cast<td::uint64>(get_int(obj, "max_answer_size", 1 << 20));
    auto v2 = get_bool(obj, "v2", false);

    td::Bits256 overlay_bits, peer_bits;
    if (overlay_bits.from_hex(overlay_hex) != 256) {
        respond_error("Invalid overlay_id hex");
        return;
    }
    if (peer_bits.from_hex(peer_hex) != 256) {
        respond_error("Invalid peer_adnl_id hex");
        return;
    }

    auto data_res = td::base64_decode(data_b64);
    if (data_res.is_error()) {
        respond_error("Invalid base64 data");
        return;
    }

    auto overlay_id = ton::overlay::OverlayIdShort{overlay_bits};
    auto peer_id = ton::adnl::AdnlNodeIdShort{peer_bits};
    auto data = td::BufferSlice(data_res.move_as_ok());
    auto timeout = td::Timestamp::in(10.0);

    auto promise = td::PromiseCreator::lambda(
        [SelfId = actor_id(this)](td::Result<td::BufferSlice> R) {
            if (R.is_error()) {
                td::actor::send_closure(SelfId, &CompatTestNode::respond_error,
                                       "RLDP query failed: " + R.error().message().str());
                return;
            }
            auto answer = R.move_as_ok();
            auto b64 = td::base64_encode(answer.as_slice());
            std::ostringstream oss;
            oss << "{\"result\": {\"answer\": \"" << b64 << "\"}}";
            td::actor::send_closure(SelfId, &CompatTestNode::respond, oss.str());
        });

    if (v2) {
        td::actor::send_closure(
            overlays_, &ton::overlay::Overlays::send_query_via,
            peer_id, local_id_short_, overlay_id, "compat_rldp_query",
            std::move(promise),
            timeout, std::move(data), max_answer_size,
            rldp2_.get());
    } else {
        td::actor::send_closure(
            overlays_, &ton::overlay::Overlays::send_query_via,
            peer_id, local_id_short_, overlay_id, "compat_rldp_query",
            std::move(promise),
            timeout, std::move(data), max_answer_size,
            rldp_.get());
    }
}

void CompatTestNode::cmd_set_query_handler(td::JsonObject &obj) {
    auto overlay_hex = get_string(obj, "overlay_id");
    auto mode = get_string(obj, "mode");

    td::Bits256 bits;
    if (bits.from_hex(overlay_hex) != 256) {
        respond_error("Invalid overlay_id hex");
        return;
    }
    auto overlay_id = ton::overlay::OverlayIdShort{bits};

    auto it = overlay_states_.find(overlay_id);
    if (it == overlay_states_.end()) {
        respond_error("Overlay not found");
        return;
    }

    it->second.query_handler_mode = mode;
    respond_ok();
}

void CompatTestNode::cmd_set_broadcast_validator(td::JsonObject &obj) {
    auto overlay_hex = get_string(obj, "overlay_id");
    auto mode = get_string(obj, "mode");

    td::Bits256 bits;
    if (bits.from_hex(overlay_hex) != 256) {
        respond_error("Invalid overlay_id hex");
        return;
    }
    auto overlay_id = ton::overlay::OverlayIdShort{bits};

    auto it = overlay_states_.find(overlay_id);
    if (it == overlay_states_.end()) {
        respond_error("Overlay not found");
        return;
    }

    it->second.broadcast_validator_mode = mode;
    respond_ok();
}

void CompatTestNode::cmd_get_received_broadcasts(td::JsonObject &obj) {
    auto overlay_hex = get_string(obj, "overlay_id");

    LOG(INFO) << "get_received_broadcasts: overlay_hex='" << overlay_hex << "' len=" << overlay_hex.length();

    td::Bits256 bits;
    auto hex_result = bits.from_hex(overlay_hex);
    if (hex_result != 256) {
        LOG(INFO) << "from_hex returned " << hex_result << " (expected 64)";
        respond_error("Invalid overlay_id hex");
        return;
    }
    auto overlay_id = ton::overlay::OverlayIdShort{bits};

    auto it = overlay_states_.find(overlay_id);
    if (it == overlay_states_.end()) {
        respond_error("Overlay not found");
        return;
    }

    std::ostringstream oss;
    oss << "{\"result\": [";
    bool first = true;
    for (auto &b : it->second.received_broadcasts) {
        if (!first) oss << ", ";
        first = false;
        oss << "{\"source\": \"" << b.source.bits256_value().to_hex() << "\""
            << ", \"size\": " << b.data.size()
            << ", \"data\": \"" << td::base64_encode(td::Slice(b.data.data(), b.data.size())) << "\""
            << ", \"timestamp\": " << b.timestamp
            << ", \"accepted\": " << (b.was_accepted ? "true" : "false")
            << "}";
    }
    oss << "]}";
    respond(oss.str());
}

void CompatTestNode::cmd_clear_received_broadcasts(td::JsonObject &obj) {
    auto overlay_hex = get_string(obj, "overlay_id");

    td::Bits256 bits;
    if (bits.from_hex(overlay_hex) != 256) {
        respond_error("Invalid overlay_id hex");
        return;
    }
    auto overlay_id = ton::overlay::OverlayIdShort{bits};

    auto it = overlay_states_.find(overlay_id);
    if (it != overlay_states_.end()) {
        it->second.received_broadcasts.clear();
    }
    respond_ok();
}

// ---------- Callback factory ----------

std::unique_ptr<TestOverlayCallback> CompatTestNode::make_overlay_callback(
    ton::overlay::OverlayIdShort overlay_id) {
    return std::make_unique<TestOverlayCallback>(
        overlay_id,
        [this, overlay_id](ton::PublicKeyHash src, td::BufferSlice data) {
            on_broadcast_received(overlay_id, src, std::move(data));
        },
        [this, overlay_id](ton::adnl::AdnlNodeIdShort src, td::BufferSlice data,
                          td::Promise<td::BufferSlice> promise) {
            on_query_received(overlay_id, src, std::move(data), std::move(promise));
        },
        [this, overlay_id](ton::PublicKeyHash src, td::BufferSlice data,
                          td::Promise<td::Unit> promise) {
            on_check_broadcast(overlay_id, src, std::move(data), std::move(promise));
        },
        [this, overlay_id](ton::adnl::AdnlNodeIdShort src, td::BufferSlice data) {
            on_message_received(overlay_id, src, std::move(data));
        });
}

// ---------- Callback handlers ----------

void CompatTestNode::on_broadcast_received(ton::overlay::OverlayIdShort overlay_id,
                                           ton::PublicKeyHash source,
                                           td::BufferSlice data) {
    auto it = overlay_states_.find(overlay_id);
    if (it == overlay_states_.end()) return;

    ReceivedBroadcast record;
    record.source = source;
    record.overlay_id = overlay_id;
    auto slice = data.as_slice();
    record.data = std::vector<td::uint8>(slice.ubegin(), slice.uend());
    record.timestamp = static_cast<td::int32>(td::Clocks::system());
    record.was_accepted = true;

    it->second.received_broadcasts.push_back(std::move(record));
}

void CompatTestNode::on_query_received(ton::overlay::OverlayIdShort overlay_id,
                                       ton::adnl::AdnlNodeIdShort src,
                                       td::BufferSlice data,
                                       td::Promise<td::BufferSlice> promise) {
    auto it = overlay_states_.find(overlay_id);
    if (it == overlay_states_.end()) {
        promise.set_error(td::Status::Error("Overlay not found"));
        return;
    }

    it->second.received_queries.emplace_back(
        src.bits256_value().to_hex(), data.size());

    auto &mode = it->second.query_handler_mode;
    if (mode == "echo") {
        promise.set_value(std::move(data));
    } else if (mode == "capabilities") {
        // Return a fixed capabilities response
        std::string caps = "compat_test_cpp_node:v1";
        promise.set_value(td::BufferSlice(caps));
    } else if (mode == "reject") {
        promise.set_error(td::Status::Error("Rejected by test query handler"));
    } else {
        // Default echo
        promise.set_value(std::move(data));
    }
}

void CompatTestNode::on_check_broadcast(ton::overlay::OverlayIdShort overlay_id,
                                        ton::PublicKeyHash source,
                                        td::BufferSlice data,
                                        td::Promise<td::Unit> promise) {
    auto it = overlay_states_.find(overlay_id);
    if (it == overlay_states_.end()) {
        promise.set_value(td::Unit());
        return;
    }

    auto &mode = it->second.broadcast_validator_mode;
    if (mode == "accept_all") {
        promise.set_value(td::Unit());
    } else if (mode == "reject_all") {
        // Record the rejection
        ReceivedBroadcast record;
        record.source = source;
        record.overlay_id = overlay_id;
        auto slice = data.as_slice();
        record.data = std::vector<td::uint8>(slice.ubegin(), slice.uend());
        record.timestamp = static_cast<td::int32>(td::Clocks::system());
        record.was_accepted = false;
        it->second.received_broadcasts.push_back(std::move(record));

        promise.set_error(td::Status::Error("Rejected by test broadcast validator"));
    } else {
        // Default: accept
        promise.set_value(td::Unit());
    }
}

// ---------- Message commands ----------

void CompatTestNode::on_message_received(ton::overlay::OverlayIdShort overlay_id,
                                          ton::adnl::AdnlNodeIdShort source,
                                          td::BufferSlice data) {
    auto it = overlay_states_.find(overlay_id);
    if (it == overlay_states_.end()) return;

    ReceivedMessage record;
    record.source = source;
    record.overlay_id = overlay_id;
    auto slice = data.as_slice();
    record.data = std::vector<td::uint8>(slice.ubegin(), slice.uend());
    record.timestamp = static_cast<td::int32>(td::Clocks::system());

    it->second.received_messages.push_back(std::move(record));
    LOG(INFO) << "Message recorded for overlay " << overlay_id.bits256_value().to_hex()
              << " from " << source.bits256_value().to_hex()
              << " size=" << it->second.received_messages.back().data.size();
}

void CompatTestNode::cmd_send_message(td::JsonObject &obj) {
    auto overlay_hex = get_string(obj, "overlay_id");
    auto peer_hex = get_string(obj, "peer_adnl_id");
    auto data_b64 = get_string(obj, "data");

    td::Bits256 overlay_bits, peer_bits;
    if (overlay_bits.from_hex(overlay_hex) != 256) {
        respond_error("Invalid overlay_id hex");
        return;
    }
    if (peer_bits.from_hex(peer_hex) != 256) {
        respond_error("Invalid peer_adnl_id hex");
        return;
    }
    if (data_b64.empty()) {
        respond_error("Missing 'data'");
        return;
    }

    auto data_res = td::base64_decode(data_b64);
    if (data_res.is_error()) {
        respond_error("Invalid base64 data");
        return;
    }

    auto overlay_id = ton::overlay::OverlayIdShort{overlay_bits};
    auto peer_id = ton::adnl::AdnlNodeIdShort{peer_bits};
    auto data = td::BufferSlice(data_res.move_as_ok());

    LOG(INFO) << "Sending message to overlay " << overlay_id.bits256_value().to_hex()
              << " peer=" << peer_id.bits256_value().to_hex()
              << " size=" << data.size();

    td::actor::send_closure(overlays_, &ton::overlay::Overlays::send_message,
                           peer_id, local_id_short_, overlay_id, std::move(data));
    respond_ok();
}

void CompatTestNode::cmd_get_received_messages(td::JsonObject &obj) {
    auto overlay_hex = get_string(obj, "overlay_id");

    td::Bits256 bits;
    if (bits.from_hex(overlay_hex) != 256) {
        respond_error("Invalid overlay_id hex");
        return;
    }
    auto overlay_id = ton::overlay::OverlayIdShort{bits};

    auto it = overlay_states_.find(overlay_id);
    if (it == overlay_states_.end()) {
        respond_error("Overlay not found");
        return;
    }

    std::ostringstream oss;
    oss << "{\"result\": [";
    bool first = true;
    for (auto &m : it->second.received_messages) {
        if (!first) oss << ", ";
        first = false;
        oss << "{\"source\": \"" << m.source.bits256_value().to_hex() << "\""
            << ", \"size\": " << m.data.size()
            << ", \"data\": \"" << td::base64_encode(td::Slice(m.data.data(), m.data.size())) << "\""
            << ", \"timestamp\": " << m.timestamp
            << "}";
    }
    oss << "]}";
    respond(oss.str());
}

void CompatTestNode::cmd_clear_received_messages(td::JsonObject &obj) {
    auto overlay_hex = get_string(obj, "overlay_id");

    td::Bits256 bits;
    if (bits.from_hex(overlay_hex) != 256) {
        respond_error("Invalid overlay_id hex");
        return;
    }
    auto overlay_id = ton::overlay::OverlayIdShort{bits};

    auto it = overlay_states_.find(overlay_id);
    if (it != overlay_states_.end()) {
        it->second.received_messages.clear();
    }
    respond_ok();
}

void CompatTestNode::cmd_compute_candidate_id_to_sign(td::JsonObject &obj) {
    auto slot = static_cast<td::int32>(get_int(obj, "slot", 0));
    auto hash_hex = get_string(obj, "hash");
    if (hash_hex.empty()) {
        respond_error("Missing 'hash' (hex)");
        return;
    }

    td::Bits256 hash_bits;
    if (hash_bits.from_hex(hash_hex) != 256) {
        respond_error("Invalid hash hex (expected 32 bytes)");
        return;
    }

    // C++ simplex/catchain signs consensus.candidateId{slot,hash} directly.
    auto tl = ton::create_tl_object<ton::ton_api::consensus_candidateId>(slot, hash_bits);
    auto serialized = ton::serialize_tl_object(tl, true);
    auto data_b64 = td::base64_encode(serialized.as_slice());

    respond_ok("\"data\": \"" + data_b64 + "\"");
}

// ---------- BOC Compression ----------

void CompatTestNode::cmd_compress_boc(td::JsonObject &obj) {
    auto data_b64 = get_string(obj, "data");
    auto algorithm = get_string(obj, "algorithm");

    if (data_b64.empty()) {
        respond_error("Missing 'data' (base64 standard BOC)");
        return;
    }
    if (algorithm.empty()) {
        algorithm = "baseline";
    }

    auto data_res = td::base64_decode(data_b64);
    if (data_res.is_error()) {
        respond_error("Invalid base64 data: " + data_res.error().message().str());
        return;
    }
    auto data = data_res.move_as_ok();

    // Deserialize standard BOC to cell roots
    auto roots_res = vm::std_boc_deserialize_multi(td::Slice(data));
    if (roots_res.is_error()) {
        respond_error("Failed to deserialize BOC: " + roots_res.error().message().str());
        return;
    }
    auto roots = roots_res.move_as_ok();

    // Determine algorithm
    vm::CompressionAlgorithm algo;
    if (algorithm == "baseline") {
        algo = vm::CompressionAlgorithm::BaselineLZ4;
    } else if (algorithm == "improved") {
        algo = vm::CompressionAlgorithm::ImprovedStructureLZ4;
    } else {
        respond_error("Unknown algorithm: " + algorithm + " (use 'baseline' or 'improved')");
        return;
    }

    // Compress
    auto compressed_res = vm::boc_compress(roots, algo);
    if (compressed_res.is_error()) {
        respond_error("Compression failed: " + compressed_res.error().message().str());
        return;
    }
    auto compressed = compressed_res.move_as_ok();
    auto compressed_b64 = td::base64_encode(compressed.as_slice());

    respond_ok("\"compressed\": \"" + compressed_b64 + "\"");
}

void CompatTestNode::cmd_decompress_boc(td::JsonObject &obj) {
    auto data_b64 = get_string(obj, "data");
    auto max_size = static_cast<int>(get_int(obj, "max_size", 10 * 1024 * 1024));

    if (data_b64.empty()) {
        respond_error("Missing 'data' (base64 compressed BOC)");
        return;
    }

    auto data_res = td::base64_decode(data_b64);
    if (data_res.is_error()) {
        respond_error("Invalid base64 data: " + data_res.error().message().str());
        return;
    }
    auto data = data_res.move_as_ok();

    // Decompress
    auto roots_res = vm::boc_decompress(td::Slice(data), max_size);
    if (roots_res.is_error()) {
        respond_error("Decompression failed: " + roots_res.error().message().str());
        return;
    }
    auto roots = roots_res.move_as_ok();

    // Re-serialize as standard BOC
    auto boc_res = vm::std_boc_serialize_multi(std::move(roots), 2);
    if (boc_res.is_error()) {
        respond_error("BOC re-serialization failed: " + boc_res.error().message().str());
        return;
    }
    auto boc = boc_res.move_as_ok();
    auto boc_b64 = td::base64_encode(boc.as_slice());

    respond_ok("\"boc\": \"" + boc_b64 + "\"");
}

// ---------- QUIC commands ----------

void CompatTestNode::cmd_enable_quic(td::JsonObject &obj) {
    if (!quic_.empty()) {
        respond_error("QUIC already enabled");
        return;
    }

    auto peer_table = td::actor::actor_dynamic_cast<ton::adnl::AdnlPeerTable>(adnl_.get());
    if (peer_table.empty()) {
        respond_error("ADNL peer table not available");
        return;
    }

    quic_ = td::actor::create_actor<ton::quic::QuicSender>("QuicSender", peer_table, keyring_.get());
    td::actor::send_closure(quic_, &ton::quic::QuicSender::add_id, local_id_short_);

    auto quic_port = config_.udp_port + 1000;
    LOG(INFO) << "QUIC enabled, listening on port " << quic_port;

    respond_ok("\"quic_port\": " + std::to_string(quic_port));
}

void CompatTestNode::cmd_send_quic_message(td::JsonObject &obj) {
    if (quic_.empty()) {
        respond_error("QUIC not enabled. Call enable_quic first");
        return;
    }

    auto peer_hex = get_string(obj, "peer_adnl_id");
    auto data_b64 = get_string(obj, "data");

    td::Bits256 peer_bits;
    if (peer_bits.from_hex(peer_hex) != 256) {
        respond_error("Invalid peer_adnl_id hex");
        return;
    }

    auto data_res = td::base64_decode(data_b64);
    if (data_res.is_error()) {
        respond_error("Invalid base64 data");
        return;
    }

    auto peer_id = ton::adnl::AdnlNodeIdShort{peer_bits};
    auto data = td::BufferSlice(data_res.move_as_ok());

    td::actor::send_closure(quic_, &ton::quic::QuicSender::send_message,
                           local_id_short_, peer_id, std::move(data));

    respond_ok();
}

void CompatTestNode::cmd_send_quic_query(td::JsonObject &obj) {
    if (quic_.empty()) {
        respond_error("QUIC not enabled. Call enable_quic first");
        return;
    }

    auto peer_hex = get_string(obj, "peer_adnl_id");
    auto data_b64 = get_string(obj, "data");
    auto timeout_ms = get_int(obj, "timeout_ms", 5000);

    td::Bits256 peer_bits;
    if (peer_bits.from_hex(peer_hex) != 256) {
        respond_error("Invalid peer_adnl_id hex");
        return;
    }

    auto data_res = td::base64_decode(data_b64);
    if (data_res.is_error()) {
        respond_error("Invalid base64 data");
        return;
    }

    auto peer_id = ton::adnl::AdnlNodeIdShort{peer_bits};
    auto data = td::BufferSlice(data_res.move_as_ok());
    auto timeout = td::Timestamp::in(timeout_ms / 1000.0);

    td::actor::send_closure(
        quic_, &ton::quic::QuicSender::send_query,
        local_id_short_, peer_id, std::string("compat_test_quic_query"),
        td::PromiseCreator::lambda(
            [SelfId = actor_id(this)](td::Result<td::BufferSlice> R) {
                if (R.is_error()) {
                    td::actor::send_closure(SelfId, &CompatTestNode::respond_error,
                                           "QUIC query failed: " + R.error().message().str());
                    return;
                }
                auto answer = R.move_as_ok();
                auto b64 = td::base64_encode(answer.as_slice());
                std::ostringstream oss;
                oss << "{\"result\": {\"answer\": \"" << b64 << "\"}}";
                td::actor::send_closure(SelfId, &CompatTestNode::respond, oss.str());
            }),
        timeout, std::move(data));
}

void CompatTestNode::cmd_raptorq_encode(td::JsonObject &obj) {
    auto data_b64 = get_string(obj, "data");
    auto symbol_size_raw = get_int(obj, "symbol_size", 768);
    if (symbol_size_raw <= 0) {
        respond_error("symbol_size must be positive");
        return;
    }
    auto symbol_size = static_cast<size_t>(symbol_size_raw);

    if (data_b64.empty()) {
        respond_error("Missing 'data' (base64)");
        return;
    }

    auto data_res = td::base64_decode(data_b64);
    if (data_res.is_error()) {
        respond_error("Invalid base64 data: " + data_res.error().message().str());
        return;
    }
    auto data = td::BufferSlice(data_res.move_as_ok());

    auto encoder = td::fec::RaptorQEncoder::create(std::move(data), symbol_size);
    if (!encoder) {
        respond_error("Failed to create RaptorQ encoder");
        return;
    }

    auto params = encoder->get_parameters();

    // How many symbols to generate: all source + some repair
    auto num_repair_raw = get_int(obj, "repair_count", 0);
    auto num_repair = static_cast<size_t>(num_repair_raw < 0 ? 0 : num_repair_raw);
    auto total = params.symbols_count + num_repair;

    // Need precalc to generate repair symbols
    if (num_repair > 0) {
        encoder->prepare_more_symbols();
    }

    std::ostringstream oss;
    oss << "{\"result\": {"
        << "\"data_size\": " << params.data_size
        << ", \"symbol_size\": " << params.symbol_size
        << ", \"symbols_count\": " << params.symbols_count
        << ", \"symbols\": [";

    for (size_t i = 0; i < total; i++) {
        auto sym = encoder->gen_symbol(static_cast<td::uint32>(i));
        if (i > 0) oss << ", ";
        oss << "{\"id\": " << sym.id
            << ", \"data\": \"" << td::base64_encode(sym.data.as_slice()) << "\"}";
    }

    oss << "]}}";
    respond(oss.str());
}

void CompatTestNode::cmd_raptorq_decode(td::JsonObject &obj) {
    auto data_size_raw = get_int(obj, "data_size", 0);
    auto symbol_size_raw = get_int(obj, "symbol_size", 768);
    auto symbols_count_raw = get_int(obj, "symbols_count", 0);

    if (data_size_raw <= 0 || symbol_size_raw <= 0 || symbols_count_raw <= 0) {
        respond_error("Missing required params: data_size, symbol_size, symbols_count");
        return;
    }
    auto data_size = static_cast<size_t>(data_size_raw);
    auto symbol_size = static_cast<size_t>(symbol_size_raw);
    auto symbols_count = static_cast<size_t>(symbols_count_raw);

    // Parse symbols array from JSON
    // We need to manually walk the JSON array
    td::JsonValue *symbols_val = nullptr;
    for (auto &kv : obj.field_values_) {
        if (kv.first == "symbols") {
            symbols_val = &kv.second;
            break;
        }
    }

    if (!symbols_val || symbols_val->type() != td::JsonValue::Type::Array) {
        respond_error("Missing or invalid 'symbols' array");
        return;
    }

    td::fec::RaptorQEncoder::Parameters params;
    params.data_size = data_size;
    params.symbol_size = symbol_size;
    params.symbols_count = symbols_count;

    auto decoder_res = td::fec::RaptorQDecoder::create(params);
    if (decoder_res.is_error()) {
        respond_error("Failed to create decoder: " + decoder_res.error().message().str());
        return;
    }
    auto decoder = decoder_res.move_as_ok();

    auto &arr = symbols_val->get_array();
    for (auto &elem : arr) {
        if (elem.type() != td::JsonValue::Type::Object) {
            respond_error("Symbol must be a JSON object");
            return;
        }
        auto &sym_obj = elem.get_object();

        td::uint32 sym_id = 0;
        std::string sym_data_b64;
        for (auto &skv : sym_obj.field_values_) {
            if (skv.first == "id") {
                if (skv.second.type() == td::JsonValue::Type::Number) {
                    sym_id = static_cast<td::uint32>(td::to_integer<td::int64>(skv.second.get_number()));
                }
            } else if (skv.first == "data") {
                if (skv.second.type() == td::JsonValue::Type::String) {
                    sym_data_b64 = skv.second.get_string().str();
                }
            }
        }

        auto sym_data_res = td::base64_decode(sym_data_b64);
        if (sym_data_res.is_error()) {
            respond_error("Invalid base64 in symbol data");
            return;
        }

        td::fec::Symbol sym{sym_id, td::BufferSlice(sym_data_res.move_as_ok())};
        auto status = decoder->add_symbol(std::move(sym));
        if (status.is_error()) {
            respond_error("add_symbol failed: " + status.message().str());
            return;
        }
    }

    if (!decoder->may_try_decode()) {
        respond_error("Not enough symbols to decode");
        return;
    }

    auto decode_res = decoder->try_decode(false);
    if (decode_res.is_error()) {
        respond_error("Decode failed: " + decode_res.error().message().str());
        return;
    }

    auto decoded = decode_res.move_as_ok();
    auto decoded_b64 = td::base64_encode(decoded.data.as_slice());

    respond_ok("\"data\": \"" + decoded_b64 + "\"");
}

} // namespace compat_test

// ---------- Main ----------

int main(int argc, char** argv) {
    SET_VERBOSITY_LEVEL(verbosity_INFO);

    td::uint16 udp_port = 14000;
    std::string db_path = "/tmp/compat_test_node";

    for (int i = 1; i < argc; i++) {
        std::string arg = argv[i];
        if (arg == "--port" && i + 1 < argc) {
            udp_port = static_cast<td::uint16>(std::stoi(argv[++i]));
        } else if (arg == "--db" && i + 1 < argc) {
            db_path = argv[++i];
        } else if (arg == "--help" || arg == "-h") {
            std::cerr << "Usage: " << argv[0] << " [options]" << std::endl;
            std::cerr << "Options:" << std::endl;
            std::cerr << "  --port PORT    ADNL UDP listening port (default: 14000)" << std::endl;
            std::cerr << "  --db PATH      Database path (default: /tmp/compat_test_node)" << std::endl;
            return 0;
        }
    }

    td::actor::Scheduler scheduler({2});

    compat_test::CompatTestNode::Config config;
    config.udp_port = udp_port;
    config.db_path = db_path;

    scheduler.run_in_context([&] {
        td::actor::create_actor<compat_test::CompatTestNode>("compat_test_node", config).release();
    });

    scheduler.run();

    return 0;
}
