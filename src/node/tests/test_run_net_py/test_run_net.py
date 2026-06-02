#!/usr/bin/env python3
import argparse
import base64
import hashlib
import json
import os
import shlex
import shutil
import subprocess
import time
from pathlib import Path

import yaml

node_proc_name: str
rust_proc_suffix: str
cpp_proc_suffix: str
run_fullnode: bool
rust_nodes_count: int
cpp_nodes_count: int
nodes_count: int
ip_address: str
main_port_base: int
control_port_base: int
liteserver_port_base: int
jsonrpc_port_base: int
logs_path: Path
common_config_path: Path
work_dirs_path: Path
bins_path: Path
cpp_src_path: Path
rust_src_path: Path
cpp_log_level: int
cpp_build_command: str

# Validator permanent key lifetime used by `addpermkey` in this harness.
# `addpermkey {key} {start} {expire}` takes Unix timestamps. A hard-coded
# constant in the past silently produces an already-expired key — fine
# today only because enforcement is lenient, fragile if it tightens. Derive
# the expiry from wall-clock time so the harness stays valid over time.
VALIDATOR_KEY_LIFETIME_SECONDS = 365 * 24 * 3600


def validator_key_expire_at() -> int:
    return int(time.time()) + VALIDATOR_KEY_LIFETIME_SECONDS


def load_config() -> bool:
    global \
        node_proc_name, \
        rust_proc_suffix, \
        cpp_proc_suffix, \
        run_fullnode, \
        rust_nodes_count, \
        cpp_nodes_count, \
        nodes_count, \
        ip_address, \
        main_port_base, \
        control_port_base, \
        liteserver_port_base, \
        jsonrpc_port_base, \
        logs_path, \
        common_config_path, \
        work_dirs_path, \
        bins_path, \
        cpp_src_path, \
        rust_src_path, \
        cpp_log_level, \
        cpp_build_command

    test_root_path = Path(__file__).parent
    config_path = test_root_path / "test_run_net.json"
    if not config_path.exists():
        if os.name == "nt":
            cpp_build_command = "build-windows.bat"
        else:
            uname_result = os.uname()
            if uname_result.sysname == "Linux":
                cpp_build_command = "build-ubuntu-shared.sh"
            elif uname_result.sysname == "Darwin":
                cpp_build_command = "build-macos-shared.sh"
            else:
                cpp_build_command = ""
                print(
                    "Unknown OS, please set cpp_build_command manually in the config file"
                )

        default_config = {
            "node_proc_name": "node_singlehost",
            "rust_proc_suffix": "rs",
            "cpp_proc_suffix": "cpp",
            "run_fullnode": False,
            "rust_nodes_count": 5,
            "cpp_nodes_count": 1,
            "ip_address": "127.0.0.1",
            "main_port_base": 3000,
            "control_port_base": 3100,
            "liteserver_port_base": 3200,
            "jsonrpc_port_base": 3300,
            "logs_path": str(test_root_path / "tmp"),
            "common_config_path": str(test_root_path / "tmp"),
            "work_dirs_path": str(test_root_path / "tmp"),
            "bins_path": str(test_root_path / "tmp" / "bins"),
            "rust_src_path": str(test_root_path.parent.parent.parent),
            "cpp_src_path": str(
                test_root_path.parent.parent.parent.parent.parent
                / "ton-blockchain"
                / "ton"
            ),
            "cpp_log_level": 4,
            "cpp_build_command": cpp_build_command,
        }
        with open(config_path, "w") as f:
            json.dump(default_config, f, indent=2)

        print(
            f"Config file {config_path} with default parameters was created, please edit it if needed and run the script again."
        )
        return False

    with open(config_path) as f:
        config = json.load(f)

    node_proc_name = config["node_proc_name"]
    rust_proc_suffix = config["rust_proc_suffix"]
    cpp_proc_suffix = config["cpp_proc_suffix"]
    run_fullnode = config["run_fullnode"]
    rust_nodes_count = config["rust_nodes_count"]
    cpp_nodes_count = config["cpp_nodes_count"]
    nodes_count = rust_nodes_count + cpp_nodes_count
    ip_address = config["ip_address"]
    main_port_base = config["main_port_base"]
    control_port_base = config["control_port_base"]
    liteserver_port_base = config["liteserver_port_base"]
    jsonrpc_port_base = config["jsonrpc_port_base"]
    logs_path = Path(config["logs_path"])
    common_config_path = Path(config["common_config_path"])
    work_dirs_path = Path(config["work_dirs_path"])
    bins_path = Path(config["bins_path"])
    cpp_src_path = Path(config["cpp_src_path"])
    rust_src_path = Path(config["rust_src_path"])
    cpp_log_level = config["cpp_log_level"]
    cpp_build_command = config["cpp_build_command"]

    print(f"Rust node process name: {node_proc_name + '_' + rust_proc_suffix}")
    print(f"C++ node process name: {node_proc_name + '_' + cpp_proc_suffix}")
    print(f"Rust nodes count: {rust_nodes_count}")
    print(f"C++ nodes count: {cpp_nodes_count}")
    print(f"Run fullnode: {run_fullnode}")
    print(f"IP address: {ip_address}")
    print(f"Main port base: {main_port_base}")
    print(f"Control port base: {control_port_base}")
    print(f"Liteserver port base: {liteserver_port_base}")
    print(f"jsonRPC port base: {jsonrpc_port_base}")
    print(f"Logs path: {logs_path}")
    print(f"Common config path: {common_config_path}")
    print(f"Node working dirs path: {work_dirs_path}")
    print(f"Bins path: {bins_path}")
    print(f"C++ sources path: {cpp_src_path}")
    print(f"Rust sources path: {rust_src_path}")
    print(f"C++ log level: {cpp_log_level}")
    print(f"C++ build command: {cpp_build_command}")

    return True


def print_current_branches():
    current_branch = run_command(
        ["git", "branch", "--show-current"], rust_src_path
    ).stdout.strip()
    print(f"Current rust branch: {current_branch}")
    if cpp_nodes_count > 0:
        current_branch_cpp = run_command(
            ["git", "branch", "--show-current"], cpp_src_path
        ).stdout.strip()
        print(f"Current C++ branch: {current_branch_cpp}")


def run_command(
    cmd: list[str],
    cwd: Path | None = None,
    check: bool = True,
    capture_output: bool = True,
):
    if cwd:
       print(f"$ (in {cwd}) {shlex.join(cmd)}")
    else:
       print(f"$ {shlex.join(cmd)}")

    try:
        result = subprocess.run(
            cmd,
            cwd=cwd,
            check=check,
            capture_output=capture_output,
            text=True,
        )
    except subprocess.CalledProcessError as e:
        raise RuntimeError(
            f"Command {cmd} failed \ncode: {e.returncode}\nstderr: {e.stderr} \nstdout: {e.stdout}"
        )
    return result


def kill_nodes(procname: str):
    try:
        print(f"Killing existing {procname} processes...", end="")
        run_command(["pkill", "-9", procname])
        print(" done")
    except RuntimeError:
        print(" no node processes found")


def cleanup():
    print("Cleaning up...", end="")
    for name in ["zerostate.json", "global_config.json"]:
        try:
            os.remove(common_config_path / name)
        except FileNotFoundError:
            pass
    for n in range(0 if run_fullnode else 1, nodes_count + 1):
        shutil.rmtree(build_node_work_path(n), ignore_errors=True)
        shutil.rmtree(work_dirs_path / f"node_db_{n}", ignore_errors=True)
    for file in Path(common_config_path).glob("*.boc"):
        file.unlink()
    for file in Path(logs_path).glob("*.log"):
        file.unlink()
    try:
        os.remove(logs_path / "nodes.txt")
    except FileNotFoundError:
        pass
    print(" done")


def build_rust(build_features: list[str] = []):
    print("Building rust binaries...")
    cmd = ["cargo", "build", "--release"] + (
        ["--features", ",".join(build_features)] if build_features else []
    )
    run_command(cmd, rust_src_path, capture_output=False)
    release_path = rust_src_path / "target" / "release"
    bins_path.mkdir(parents=True, exist_ok=True)
    shutil.copy(
        release_path / "node",
        bins_path / (node_proc_name + "_" + rust_proc_suffix),
    )
    shutil.copy(release_path / "console", bins_path / "console")
    shutil.copy(release_path / "crypto", bins_path / "crypto")
    shutil.copy(release_path / "zerostate", bins_path / "zerostate")
    print(" done")


def build_cpp():

    # TODO: implement cross-platform compatibility

    print("Building C++ node binary...")
    run_command(["bash", cpp_build_command], cwd=cpp_src_path)
    bins_path.mkdir(parents=True, exist_ok=True)
    shutil.copy(
        cpp_src_path / "build" / "validator-engine" / "validator-engine",
        bins_path / (node_proc_name + "_" + cpp_proc_suffix),
    )
    shutil.copy(
        cpp_src_path
        / "build"
        / "validator-engine-console"
        / "validator-engine-console",
        bins_path / "cpp_console",
    )
    print(" done")


def build_node_work_path(node_index: int) -> Path:
    return work_dirs_path / f"node_{node_index}"


def prepare_default_config(
    node_index: int, config_blank: str, log_config_blank: str,
    use_quic: bool = False, quic_port_offset: int = 1000,
):
    node_work_path = build_node_work_path(node_index)
    node_work_path.mkdir(parents=True, exist_ok=True)

    print(f"Preparing log config for node {node_index}...", end="")
    # by default off without quotes is treated as boolean false in yaml
    log_config_blank = log_config_blank.replace("off", '"off"')
    log_cfg = yaml.safe_load(log_config_blank)
    log_cfg["appenders"]["rolling_logfile"]["path"] = str(
        logs_path / f"output_{node_index}.log"
    )
    log_cfg["appenders"]["rolling_logfile"]["policy"]["roller"]["pattern"] = str(
        logs_path / f"output_{node_index}_{{}}.log"
    )
    with open(node_work_path / "log_cfg.yml", "w") as f:
        yaml.safe_dump(log_cfg, f)
    print(" done")

    print(f"Generating default config for node {node_index}...", end="")
    config = json.loads(config_blank)
    config["log_config_name"] = str(node_work_path / "log_cfg.yml")
    config["ton_global_config_name"] = str(common_config_path / "global_config.json")
    config["internal_db_path"] = str(node_work_path)
    adnl_port = main_port_base + node_index
    config["ip_address"] = f"{ip_address}:{adnl_port}"
    if use_quic:
        quic_port = adnl_port + quic_port_offset
        config["ip_address_quic"] = f"{ip_address}:{quic_port}"
    config["control_server_port"] = control_port_base + node_index
    config["lite_server_port"] = liteserver_port_base + node_index
    config["json_rpc_server"] = {"address": f"0.0.0.0:{jsonrpc_port_base + node_index}"}
    with open(node_work_path / "default_config.json", "w") as f:
        json.dump(config, f, indent=2)
    print(" done")


def run_rust_node(
    params: list[str], node_index: int, start_new_session: bool = False,
    with_vault: bool = True,
) -> subprocess.Popen:
    stdout_path = logs_path / f"stdout_{node_index}.log"
    stderr_path = logs_path / f"stderr_{node_index}.log"
    working_dir = build_node_work_path(node_index)
    node_bin_path = bins_path / (node_proc_name + "_" + rust_proc_suffix)
    cmd = [str(node_bin_path)] + params
    print(shlex.join(cmd))
    if start_new_session:
        print(f"Starting node {node_index}...")

    node_env = os.environ.copy()
    if with_vault:
        per_node_url = node_env.get(f"VAULT_URL_NODE_{node_index}")
        if per_node_url:
            node_env["VAULT_URL"] = per_node_url
    else:
        node_env.pop("VAULT_URL", None)

    with stdout_path.open("w") as out_log, stderr_path.open("w") as err_log:
        proc = subprocess.Popen(
            [str(node_bin_path)] + params,
            cwd=working_dir,
            stdout=out_log,
            stderr=err_log,
            start_new_session=start_new_session,
            env=node_env,
        )
    return proc


def run_cpp_node(
    params: list[str], node_index: int, start_new_session: bool = False
) -> subprocess.Popen:
    stdout_path = logs_path / f"output_{node_index}.log"
    stderr_path = logs_path / f"output_{node_index}.log"
    working_dir = build_node_work_path(node_index)
    node_bin_path = bins_path / (node_proc_name + "_" + cpp_proc_suffix)
    if not node_bin_path.exists():
        raise FileNotFoundError(
            f"C++ binary not found at {node_bin_path}. "
            f"Either build it or copy it to {bins_path}/ before running with cpp_nodes_count > 0."
        )
    if start_new_session:
        print(f"Starting C++ node {node_index}...")
    with stdout_path.open("w") as out_log, stderr_path.open("w") as err_log:
        proc = subprocess.Popen(
            [
                str(node_bin_path),
                "-D",
                ".",
                "-C",
                str(common_config_path / "global_config.json"),
                "--verbosity",
                str(cpp_log_level),
            ]
            + params,
            cwd=working_dir,
            stdout=out_log,
            stderr=err_log,
            start_new_session=start_new_session,
        )
    return proc


def run_console(params: list[str], node_index: int, config_path: str | Path) -> str:
    # print(f"Starting console with params {params} for node {node_index}...")
    console_bin_path = bins_path / "console"
    cmd = [str(console_bin_path), "-C", str(config_path)] + params
    proc = run_command(cmd, cwd=bins_path)
    # Sometimes console could not connect to node next time without this delay
    time.sleep(1)
    return proc.stdout.strip()


def wait_control_server(node_index: int, console_config_path: str | Path):
    print(f"Waiting for control server of node {node_index}...", end="")
    timeout = 0.1
    for _ in range(10):
        time.sleep(timeout)
        try:
            run_console(["-c", "getstats"], node_index, console_config_path)
            print(" done")
            return
        except RuntimeError:
            timeout *= 2
    raise RuntimeError(f"Control server not responding for node {node_index}")


def generate_validator_key(node_index: int, console_config_path: str | Path) -> str:
    print(f"Generating validator key for node {node_index}...", end="")
    params = ["-c", "newkey"]
    output = run_console(params, node_index, console_config_path)
    output_lines = output.split("\n")
    key = None
    for line in output_lines:
        if "received public key hash:" in line:
            key = line.split("received public key hash:")[1].strip().split(" ")[1]
            break
    if key is None:
        raise RuntimeError(f"Failed to find validator key for node {node_index}")
    # print(f"Validator key for node {node_index}: {key}")
    print(" done")
    return key


def import_validator_key(node_index: int, console_config_path: str | Path, key: str):
    print(f"Adding validator key for node {node_index}...", end="")
    params = ["-c", f"addpermkey {key} {int(time.time())} {validator_key_expire_at()}"]
    run_console(params, node_index, console_config_path)
    print(" done")


def export_validator_pubkey(
    node_index: int, console_config_path: str | Path, key: str
) -> str:
    print(f"Exporting validator public key for node {node_index}...", end="")
    params = ["-c", f"exportpub {key}"]
    output = run_console(params, node_index, console_config_path)
    output_lines = output.split("\n")
    pubkey = None
    for line in output_lines:
        if "imported key:" in line:
            pubkey = line.split("imported key:")[1].strip().split(" ")[0]
            break
    if pubkey is None:
        raise RuntimeError(f"Failed to find validator pubkey for node {node_index}")
    print(" done")
    return pubkey


def prepare_node(
    node_index: int, config_blank: str, log_config_blank: str,
    use_quic: bool = False, quic_port_offset: int = 1000,
) -> str | None:

    # Prepare console key
    keygen_result = run_command([str(bins_path / "crypto"), "gen", "key"], cwd=bins_path)
    console_key_json = json.loads(keygen_result.stdout)

    prepare_default_config(
        node_index, config_blank, log_config_blank,
        use_quic=use_quic, quic_port_offset=quic_port_offset,
    )

    #  Run node
    console_public = {"type_id": 1209251014, "pub_key": console_key_json["pubkey"]}
    params = ["--configs", ".", "--ckey", json.dumps(console_public)]
    print(f"Starting node {node_index} to generate configs...")
    node_proc = run_rust_node(params, node_index, with_vault=False)
    try:
        node_work_path = build_node_work_path(node_index)

        # Wait for node to start and generate configs
        config_json_path = node_work_path / "config.json"
        timeout = 0.1
        for attempt in range(5):
            time.sleep(timeout)
            if config_json_path.exists():
                break
            else:
                if attempt == 4:
                    raise RuntimeError(f"Config json not found for node {node_index}")
                timeout *= 2

        console_part_config_path = node_work_path / "console_config.json"
        if not console_part_config_path.exists():
            raise RuntimeError(f"Console config not found for node {node_index}")

        # Build full console config
        console_config_path = node_work_path / "console.json"
        with (
            open(console_part_config_path) as f,
            open(console_config_path, "w") as fout,
        ):
            c = json.load(f)
            c["client_key"] = {"type_id": 1209251014, "pvt_key": console_key_json["secret"]}
            console_full_config = {"config": c, "wallet_id": "", "max_factor": 3}
            json.dump(console_full_config, fout, indent=2)

        # Add validator key via console
        # (0 is full node, others are validators)
        validator_pubkey_hex = None
        if node_index != 0:
            wait_control_server(node_index, console_config_path)
            key = generate_validator_key(node_index, console_config_path)
            import_validator_key(node_index, console_config_path, key)
            validator_pubkey_hex = export_validator_pubkey(
                node_index, console_config_path, key
            )

        config = json.loads(Path(node_work_path / "config.json").read_text())
        _, fullnode_pvt_key = extract_keys_from_rust_config(config)
        with open(logs_path / f"nodes.txt", "a") as f:
            f.write(f"Node {node_index}:\n")
            f.write(f"  ip endpoint: {ip_address}:{main_port_base + node_index}\n")
            adnl_id_b64 = calc_key_id_from_pvtkey(fullnode_pvt_key)
            adnl_id_hex = base64.b64decode(adnl_id_b64).hex().lower()
            f.write(f"  node adnl id: {adnl_id_b64} {adnl_id_hex}\n")
            if validator_pubkey_hex is not None:
                validator_pubkey_b64 = base64.b64encode(
                    bytes.fromhex(validator_pubkey_hex)
                ).decode("utf-8")
                validator_adnl_id = calc_key_id_from_pubkey(validator_pubkey_b64)
                validator_adnl_id_hex = base64.b64decode(validator_adnl_id).hex().lower()
                f.write(f"  validator adnl id: {validator_adnl_id} {validator_adnl_id_hex}\n")

    finally:
        # Stop node process
        node_proc.terminate()
        node_proc.wait()

    print(f"Node {node_index} prepared")

    return validator_pubkey_hex


def extract_keys_from_rust_config(rust_config: dict):
    dht_pvt_key = None
    fullnode_pvt_key = None
    for key in rust_config["adnl_node"]["keys"]:
        if key["tag"] == 1:  # 1 is DHT tag
            dht_pvt_key = key["data"]["pvt_key"]
        if key["tag"] == 2:  # 2 is fullnode tag
            fullnode_pvt_key = key["data"]["pvt_key"]
    if dht_pvt_key is None:
        raise RuntimeError(f"DHT key not found in rust config")
    if fullnode_pvt_key is None:
        raise RuntimeError(f"Fullnode key not found in rust config")
    return dht_pvt_key, fullnode_pvt_key


def transform_configs_for_cpp(node_index: int, use_quic: bool = False, quic_port_offset: int = 1000):
    print(f"Transforming configs for C++ node {node_index}...", end="")

    node_work_path = build_node_work_path(node_index)
    rust_config = json.loads(Path(node_work_path / "config.json").read_text())
    rust_console_config = json.loads(Path(node_work_path / "console.json").read_text())
    shutil.rmtree(node_work_path)
    node_work_path.mkdir(parents=True, exist_ok=True)

    # run cpp node to generate config
    node_proc = run_cpp_node(
        ["--ip", f"{ip_address}:{main_port_base + node_index}"], node_index
    )
    node_proc.wait()

    # load cpp config
    cpp_config = json.loads(Path(node_work_path / "config.json").read_text())

    # take DHT and fullnode keys from rust
    dht_pvt_key, fullnode_pvt_key = extract_keys_from_rust_config(rust_config)

    # take validator key from rust

    expire_at = rust_config["validator_keys"][0]["expire_at"]
    election_id = rust_config["validator_keys"][0]["election_id"]
    validator_key_id = rust_config["validator_keys"][0]["validator_key_id"]
    validator_pvt_key = rust_config["validator_key_ring"][validator_key_id]["pvt_key"]
    liteserver_pvt_key = rust_config["lite_server"]["server_key"]["pvt_key"]

    crypto_tool_path = bins_path / "crypto"
    cmd = [str(crypto_tool_path), "gen", "dht", "--addr", f"127.0.0.1:1", "--key", liteserver_pvt_key]
    ls_node = json.loads(run_command(cmd, cwd=common_config_path).stdout.strip())
    liteserver_pubkey_b64 = ls_node["id"]["key"]
    liteserver_key_id_b64 = calc_key_id_from_pubkey(liteserver_pubkey_b64)
    ls_port = int(rust_config["lite_server"]["address"].rsplit(":", 1)[1])

    # take console keys from rust
    console_srv_secret_b64 = rust_config["control_server"]["server_key"]["pvt_key"]
    console_srv_pub_b64 = rust_console_config["config"]["server_key"]["pub_key"]
    console_srv_id = calc_key_id_from_pubkey(console_srv_pub_b64)
    console_secret_b64 = rust_console_config["config"]["client_key"]["pvt_key"]
    console_pub_b64 = rust_config["control_server"]["clients"]["list"][0]["pub_key"]
    console_id = calc_key_id_from_pubkey(console_pub_b64)

    # fill validators config

    cpp_config["liteservers"] = [
        {
            "@type": "engine.liteServer",
            "id": liteserver_key_id_b64,
            "port": ls_port,
        }
    ]

    cpp_config["validators"] = [
        {
            "@type": "engine.validator",
            "id": validator_key_id,
            "election_date": election_id,
            "expire_at": expire_at,
            "adnl_addrs": [
                {
                    "@type": "engine.validatorAdnlAddress",
                    "id": validator_key_id,
                    "expire_at": expire_at,
                }
            ],
            "temp_keys": [
                {
                    "@type": "engine.validatorTempKey",
                    "key": validator_key_id,
                    "expire_at": expire_at,
                }
            ],
        }
    ]

    crypto_tool_path = bins_path / "crypto"
    port = main_port_base + node_index

    # DHT key (use crypto gen dht to make public key from private)
    cmd = [str(crypto_tool_path), "gen", "dht", "--addr", f"{ip_address}:{port}", "--key", dht_pvt_key]
    node = json.loads(run_command(cmd, cwd=common_config_path).stdout.strip())
    dht_key_id_b64 = calc_key_id_from_pubkey(node["id"]["key"])
    cpp_config["dht"] = [{"@type": "engine.dht", "id": dht_key_id_b64}]

    # fullnode key
    cmd = [str(crypto_tool_path), "gen", "dht", "--addr", f"{ip_address}:{port}", "--key", fullnode_pvt_key]
    node = json.loads(run_command(cmd, cwd=common_config_path).stdout.strip())
    fullnode_key_id_b64 = calc_key_id_from_pubkey(node["id"]["key"])
    cpp_config["fullnode"] = fullnode_key_id_b64

    cpp_config["adnl"] = [
        {"@type": "engine.adnl", "id": dht_key_id_b64, "category": 0},
        {"@type": "engine.adnl", "id": fullnode_key_id_b64, "category": 1},
        {"@type": "engine.adnl", "id": validator_key_id, "category": 0},
        {"@type": "engine.adnl", "id": liteserver_key_id_b64, "category": 2},
    ]

    cpp_config["control"] = [
        {
            "id": console_srv_id,
            "port": control_port_base + node_index,
            "allowed": [
                {"id": console_id, "permissions": 15}
            ],
        }
    ]

    # save keys to the file db-path/keyring/key-id
    add_to_cpp_keyring(
        node_index, validator_pvt_key, base64.b64decode(validator_key_id)
    )
    add_to_cpp_keyring(node_index, fullnode_pvt_key, base64.b64decode(fullnode_key_id_b64))
    add_to_cpp_keyring(node_index, dht_pvt_key, base64.b64decode(dht_key_id_b64))
    add_to_cpp_keyring(node_index, console_srv_secret_b64, base64.b64decode(console_srv_id))
    add_to_cpp_keyring(node_index, liteserver_pvt_key, base64.b64decode(liteserver_key_id_b64))

    # add QUIC address if enabled
    if use_quic:
        import ipaddress
        adnl_port = main_port_base + node_index
        quic_port = adnl_port + quic_port_offset
        ip_int = int(ipaddress.IPv4Address(ip_address))
        cpp_config.setdefault("addrs", []).append({
            "@type": "engine.quicAddr",
            "ip": ip_int,
            "port": quic_port,
            "categories": [0, 1, 2, 3],
            "priority_categories": [],
        })

    # save modified cpp config
    with open(node_work_path / "config.json", "w") as f:
        json.dump(cpp_config, f, indent=2)

    liteclient_config = {
        "client_key": None,
        "server_address": f"127.0.0.1:{ls_port}",
        "server_key": {
            "type_id": 1209251014,
            "pub_key": liteserver_pubkey_b64,
        },
    }

    with open(node_work_path / "lite_client_config.json", "w") as f:
        json.dump(liteclient_config, f, indent=2)
    # copy zerostates
    statis_path = node_work_path / "static"
    statis_path.mkdir(parents=True, exist_ok=True)
    for boc_file in common_config_path.glob("*.boc"):
        shutil.copy(boc_file, statis_path / boc_file.stem.upper())

    # save client's keys
    with open(node_work_path / "client", "wb") as f:
        f.write(bytes.fromhex("17236849") + base64.b64decode(console_secret_b64))
    with open(node_work_path / "server.pub", "wb") as f:
        f.write(bytes.fromhex("c6b41348") + base64.b64decode(console_srv_pub_b64))
    with open(node_work_path / "console.sh", "wb") as f:
        console_path = bins_path / "cpp_console"
        script = f"{console_path} -k client -p server.pub -a {ip_address}:{control_port_base + node_index}"
        f.write(script.encode("utf-8"))

    print(" done")


def add_to_cpp_keyring(node_index: int, pvt_key: str, key_id: bytes):
    keyring_path = build_node_work_path(node_index) / "keyring"
    keyring_path.mkdir(parents=True, exist_ok=True)
    key_id_hex = key_id.hex().upper()

    with open(keyring_path / key_id_hex, "wb") as f:
        f.write(bytes.fromhex("17236849") + base64.b64decode(pvt_key))


def calc_key_id_from_pvtkey(pvt_key_b64: str) -> str:
    cmd = [str(bins_path / "crypto"), "get", "adnl-id", "--secret", pvt_key_b64]
    return json.loads(run_command(cmd, cwd=common_config_path).stdout.strip())["adnlId"]


def calc_key_id_from_pubkey(pub_key_b64: str) -> str:
    cmd = [str(bins_path / "crypto"), "get", "adnl-id", "--public", pub_key_b64]
    return json.loads(run_command(cmd, cwd=common_config_path).stdout.strip())["adnlId"]


def build_zerostate(
    zerostate_blank: str,
    validator_pub_key_hex: list[str],
    simplex_mc: bool = False,
    simplex_config: dict = None,
    use_quic: bool = False,
    enable_observers: bool = False,
) -> str:
    print("Building zerostate...", end="")
    zerostate = json.loads(zerostate_blank)
    now = int(time.time())
    elected_for = zerostate["master"]["config"]["p15"]["validators_elected_for"]
    zerostate["gen_utime"] = now
    zerostate["master"]["config"]["p12"][0]["enabled_since"] = now
    zerostate["master"]["config"]["p34"]["utime_since"] = now
    zerostate["master"]["config"]["p34"]["utime_until"] = now + elected_for
    zerostate["master"]["config"]["p34"]["total"] = len(validator_pub_key_hex)
    zerostate["master"]["config"]["p34"]["main"] = len(validator_pub_key_hex)
    zerostate["master"]["config"]["p34"]["total_weight"] = (
        len(validator_pub_key_hex) * 10
    )
    validators = []
    for pubkey in validator_pub_key_hex:
        validator_entry = {
            "public_key": pubkey,
            "weight": "10",
        }
        validators.append(validator_entry)
    zerostate["master"]["config"]["p34"]["list"] = validators

    # Add ConfigParam 30 (NewConsensusConfigAll) for simplex if enabled
    if simplex_config:
        # Simplex (C++/Rust) allows equal `gen_utime` only starting from global_version >= 13.
        # Our default zerostate template uses version=11, which forces strict `prev + 1` and
        # makes fast single-host nets drift into the future, triggering validation rejects.
        #
        # Keep behavior C++-compatible by bumping version to at least 13 when simplex is enabled.
        zerostate["master"]["config"]["p8"]["version"] = max(
            int(zerostate["master"]["config"]["p8"].get("version", 0)),
            13,
        )

        p30 = {}
        simplex_entry = {
            "target_rate_ms": simplex_config.get("target_rate_ms", 500),
            "slots_per_leader_window": simplex_config.get("slots_per_leader_window", 4),
            "first_block_timeout_ms": simplex_config.get(
                "first_block_timeout_ms", 1000
            ),
            "max_leader_window_desync": simplex_config.get(
                "max_leader_window_desync", 2
            ),
        }
        if use_quic:
            simplex_entry["use_quic"] = 1
        # Route block-candidate broadcasts through the dedicated block-sync overlay
        if enable_observers:
            simplex_entry["enable_observers"] = 1
        # MC simplex config (enabled when --simplex-mc is specified)
        if simplex_mc:
            p30["mc"] = dict(simplex_entry)
        # Shard simplex config (always enabled when simplex is used)
        p30["shard"] = dict(simplex_entry)
        zerostate["master"]["config"]["p30"] = p30
        quic_str = ", quic=true" if use_quic else ""
        obs_str = ", enable_observers=true" if enable_observers else ""
        print(f" [simplex enabled: mc={simplex_mc}{quic_str}{obs_str}]", end="")

    zs_json_path = common_config_path / "zerostate.json"
    with zs_json_path.open("w") as fout:
        json.dump(zerostate, fout, indent=2)

    zs_tool_path = bins_path / "zerostate"
    cmd = [str(zs_tool_path), str(zs_json_path), "-o", str(common_config_path)]
    proc = run_command(cmd, cwd=common_config_path)
    # Rename output files to {hex_file_hash}.boc as expected by the node (-z flag)
    zs_info = json.loads(proc.stdout.strip())
    for entry, src_name in [("zero_state", "zerostate.boc"), ("base_state", "basestate0.boc")]:
        if entry not in zs_info:
            continue
        file_hash_b64 = zs_info[entry]["file_hash"]
        hex_hash = base64.b64decode(file_hash_b64).hex()
        src = common_config_path / src_name
        dst = common_config_path / f"{hex_hash}.boc"
        src.rename(dst)
    print(" done")
    return proc.stdout.strip()


def build_global_config(zerostate_info: str):
    print("Building global config...", end="")
    nodes = []
    liteservers = []
    for n in range(0 if run_fullnode else 1, nodes_count + 1):
        node_conf = json.loads((build_node_work_path(n) / "config.json").read_text())
        dht_key = None
        for key in node_conf["adnl_node"]["keys"]:
            if key["tag"] == 1:  # 1 is DHT tag
                dht_key = key["data"]["pvt_key"]
                break
        if dht_key is None:
            raise RuntimeError(f"DHT key not found for node {n} config")

        crypto_tool_path = bins_path / "crypto"
        port = main_port_base + n
        cmd = [str(crypto_tool_path), "gen", "dht", "--addr", f"{ip_address}:{port}", "--key", dht_key]
        node = json.loads(run_command(cmd, cwd=common_config_path).stdout.strip())
        nodes.append(node)

        # Lite server
        node_ls_conf = json.loads(
            (build_node_work_path(n) / "lite_client_config.json").read_text()
        )
        ip_str = node_ls_conf["server_address"].split(":")[0]
        ip_bytes = bytes(map(int, ip_str.split(".")))
        ip_int = int.from_bytes(ip_bytes, byteorder="big")
        liteserver = {
            "ip": ip_int,
            "port": int(node_ls_conf["server_address"].split(":")[1]),
            "id": {
                "@type": "pub.ed25519",
                "key": node_ls_conf["server_key"]["pub_key"],
            },
        }
        liteservers.append(liteserver)

    gconf = json.loads(Path(common_config_path / "global_config.json").read_text())
    gconf["dht"]["static_nodes"]["nodes"] = nodes
    gconf["validator"]["zero_state"] = json.loads(zerostate_info)["zero_state"]
    gconf["liteservers"] = liteservers

    with (common_config_path / "global_config.json").open("w") as fout:
        json.dump(gconf, fout, indent=2)

    print(" done")

def add_control_client_key_to_nodes(pub_key_b64: str):
    """Add a shared control client public key to every node's control_server.clients.list."""
    global run_fullnode, nodes_count
    print("Adding shared control client public key to all nodes...", end="")
    for n in range(0 if run_fullnode else 1, nodes_count + 1):
        node_cfg_path = build_node_work_path(n) / "config.json"
        if not node_cfg_path.exists():
            print(f"\n  Warning: config.json not found for node {n}, skipping", end="")
            continue
        with open(node_cfg_path) as f:
            cfg = json.load(f)
        clients_list = cfg.get("control_server", {}).get("clients", {}).get("list", [])
        clients_list.append({"type_id": 1209251014, "pub_key": pub_key_b64})
        cfg.setdefault("control_server", {}).setdefault("clients", {})["list"] = clients_list
        with open(node_cfg_path, "w") as f:
            json.dump(cfg, f, indent=2)
    print(" done")


def build_nodectl_config(root_path):
    global run_fullnode, nodes_count, common_config_path

    print("Building nodectl config...", end="")
    node_control_servers = {}
    for n in range(0 if run_fullnode else 1, nodes_count + 1):
        node_work_path = build_node_work_path(n)
        with open(node_work_path / "console.json") as f:
            c = json.load(f)
            node_control_servers["node" + str(n)] = c["config"]
            node_control_servers["node" + str(n)]["timeouts"] = 5
    with (
        open(root_path / "nodectl_blank.json") as f,
        open(common_config_path / "nodectl-local.json", "w") as fout,
    ):
        c = json.load(f)
        # New nodectl config expects `nodes`.
        c["nodes"] = node_control_servers
        c["nodes_adnl"] = node_control_servers
        json.dump(c, fout, indent=2)
    print(" done")


def main():

    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--restart", action="store_true", help="Kill, build and start nodes"
    )
    parser.add_argument("--nobuild", action="store_true", help="No rebuild")
    parser.add_argument(
        "--elections",
        action="store_true",
        help="Use zerostate_blank_elections.json instead of zerostate_blank.json",
    )
    parser.add_argument(
        "--start",
        nargs="*",
        type=int,
        help="Start nodes (not all the network) with given numbers (whitespace separated) or all if not specified",
    )
    parser.add_argument("--stop", action="store_true", help="Only kill nodes and exit")
    parser.add_argument(
        "--prepare",
        action="store_true",
        help="Kill, build, generate configs and zerostate, but do not start nodes",
    )
    parser.add_argument(
        "--simplex",
        action="store_true",
        help="Enable simplex consensus config in zerostate (ConfigParam 30)",
    )
    parser.add_argument(
        "--simplex-mc",
        action="store_true",
        help="Enable simplex consensus for masterchain (implies --simplex)",
    )
    parser.add_argument(
        "--quic",
        action="store_true",
        help="Enable QUIC overlay transport in ConfigParam 30 (use_quic flag). Implies --simplex.",
    )
    parser.add_argument(
        "--enable-observers",
        action="store_true",
        help="Set ConfigParam 30 simplex_config_v2.enable_observers=1 (implies --simplex). "
             "Routes block-candidate broadcasts through the dedicated block-sync overlay.",
    )
    parser.add_argument(
        "--quic_custom_port",
        action="store_true",
        help="Use QUIC port offset 2000 (instead of 1000) to verify DHT announces. "
             "Nodes bind QUIC on adnl_port+2000 but the auto-derive fallback is adnl_port+1000, "
             "so QUIC connections only work if advertised addresses are used. Implies --quic.",
    )
    parser.add_argument(
        "--control-client-public-key",
        type=str,
        default=None,
        metavar="BASE64",
        help="Base64 public key to add to every node's control_server.clients.list",
    )
    args = parser.parse_args()

    # --quic_custom_port implies --quic
    if args.quic_custom_port:
        args.quic = True
    # --quic implies --simplex
    if args.quic:
        args.simplex = True
    # --simplex-mc implies --simplex
    if args.simplex_mc:
        args.simplex = True
    # --enable-observers implies --simplex (BlockSync)
    if args.enable_observers:
        args.simplex = True
    if args.start is None:
        args.start = False
    run_net = not args.stop and not args.start and not args.restart and not args.prepare
    stop = run_net or args.stop or args.restart or args.prepare
    build = not args.nobuild and (run_net or args.restart or args.prepare)
    gen_configs = run_net or args.prepare
    start = run_net or args.start or args.restart

    # Init script config
    if not load_config():
        return

    print_current_branches()
    print("")

    # Common preparations
    if stop:
        kill_nodes(node_proc_name)

    if gen_configs:
        cleanup()

    if build:
        build_rust([])  # always build rust because we need tools etc.
        if cpp_nodes_count > 0:
            build_cpp()

    test_root_path = Path(__file__).parent

    if gen_configs:
        validator_pub_keys = []

        # Prepare nodes (configs, keys, etc.)
        node_config_blank = Path(test_root_path / "config_blank_rust.json").read_text()
        log_config_blank = Path(test_root_path / "log_cfg_blank.yml").read_text()
        shutil.copy(
            test_root_path / "global_config_blank.json",
            common_config_path / "global_config.json",
        )
        quic_port_offset = 2000 if args.quic_custom_port else 1000
        for n in range(0 if run_fullnode else 1, nodes_count + 1):
            vk = prepare_node(
                n, node_config_blank, log_config_blank,
                use_quic=args.quic, quic_port_offset=quic_port_offset,
            )
            if n != 0:
                validator_pub_keys.append(vk)

        if args.control_client_public_key:
            add_control_client_key_to_nodes(args.control_client_public_key)

        build_nodectl_config(test_root_path)

        # Load simplex config if simplex is enabled
        simplex_config = None
        if args.simplex:
            simplex_config_path = test_root_path / "simplex_config.json"
            if simplex_config_path.exists():
                simplex_config = json.loads(simplex_config_path.read_text())
            else:
                # Default simplex configuration
                simplex_config = {
                    "target_rate_ms": 500,
                    "slots_per_leader_window": 4,
                    "first_block_timeout_ms": 1000,
                    "max_leader_window_desync": 2,
                }
                # Save default config for future reference
                with simplex_config_path.open("w") as f:
                    json.dump(simplex_config, f, indent=2)
                print(f"Created default simplex config: {simplex_config_path}")

        # Build zerostate
        zerostate_name = (
            "zerostate_blank_elections.json"
            if args.elections
            else "zerostate_blank.json"
        )
        zerostate_blank = Path(test_root_path / zerostate_name).read_text()
        zerostate_info = build_zerostate(
            zerostate_blank,
            validator_pub_keys,
            simplex_mc=args.simplex_mc,
            simplex_config=simplex_config,
            use_quic=args.quic,
            enable_observers=args.enable_observers,
        )

        # Build global config
        build_global_config(zerostate_info)

        # Transform configs for C++ nodes
        for node_index in range(rust_nodes_count + 1, nodes_count + 1):
            transform_configs_for_cpp(
                node_index, use_quic=args.quic, quic_port_offset=quic_port_offset,
            )

    if start:
        # Start nodes
        nodes_to_start = range(0 if run_fullnode else 1, nodes_count + 1)
        if args.start is not False and len(args.start) > 0:
            nodes_to_start = args.start
            print(f"Starting specified nodes: {nodes_to_start}")

        for node_index in nodes_to_start:
            params = ["--configs", ".", "-z", str(common_config_path)]
            if node_index <= rust_nodes_count:
                run_rust_node(params, node_index, start_new_session=True)
            else:
                run_cpp_node([], node_index, start_new_session=True)


if __name__ == "__main__":
    main()
