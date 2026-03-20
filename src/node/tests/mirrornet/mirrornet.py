import os
import subprocess
import json
import time
from pathlib import Path
import base64

config: dict = {}
mirrornet_global_config_name = "mirrornet_global_config.json"


def load_config() -> bool:

    global config

    script_root_path = Path(__file__).parent
    config_path = script_root_path / "mirrornet.json"
    if not config_path.exists():
        default_config = {
            "utils_path": str(
                script_root_path.parent.parent.parent / "target" / "release"
            ),
            "nodes": [
                {
                    "username": "automation",
                    "ip": "127.0.0.1",
                    "ssh_port": 22,
                    "node_bin_path": "/ton-node/bin/node",
                    "hardfork_tool_path": "/ton-node/bin/hardfork",
                    "node_configs_path": "/ton-node/bin",
                }
            ],
        }
        with open(config_path, "w") as f:
            json.dump(default_config, f, indent=2)

        print(
            f"Config file {config_path} with default parameters was created, please fill it and run the script again."
        )
        return False

    with open(config_path) as f:
        config = json.load(f)

    return True


def run_command(
    cmd: list[str],
    cwd: Path | None = None,
    check: bool = True,
    capture_output: bool = True,
):
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


def prepare_ssh_command(node: dict, cmd: str) -> list[str]:
    ssh = [
        "ssh",
        "-p",
        str(node["ssh_port"]),
        f"{node['username']}@{node['ip']}",
        f"sh -c '{cmd}'",
    ]
    return ssh


def stop_nodes():
    print("Stopping nodes...")
    procs = []
    for node in config["nodes"]:
        bin_name = node["node_bin_path"].split("/")[-1]
        cmd = f"pkill {bin_name}; while pgrep {bin_name} > /dev/null; do sleep 1; done;"
        ssh = prepare_ssh_command(node, cmd)
        p = subprocess.Popen(
            ssh, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True
        )
        procs.append((node, p))

    for node, p in procs:
        _out, err = p.communicate()
        if p.returncode != 0:
            print(f"  ❌ error stopping node {node['ip']}: {p.returncode}\n{err}")
            raise RuntimeError(f"Failed to stop node {node['ip']}")
        else:
            print(f"  ✅ stopped node {node['ip']}")


def update_node_configs() -> tuple[list[dict], str]:
    print("Updating node configs...")
    validator_configs = []
    election_id = int(time.time())
    for node in config["nodes"]:
        print(f"  {node['ip']}...", end="")
        # backup config.json
        backup_path = Path(node["node_configs_path"]) / "config.json.bak"
        cmd = f"cp -n {node['node_configs_path']}/config.json {backup_path}"
        ssh = prepare_ssh_command(node, cmd)
        run_command(ssh)

        # download config.json
        cmd = f"cat {node['node_configs_path']}/config.json"
        ssh = prepare_ssh_command(node, cmd)
        result = run_command(ssh)
        config_json = json.loads(result.stdout)

        # generate new keypair
        result = run_command([config["utils_path"] + "/keygen"], capture_output=True)
        new_key = json.loads(result.stdout)
        config_json["validator_keys"] = [
            {
                "expire_at": int(time.time()) + 365 * 24 * 3600,
                "election_id": election_id,
                "validator_key_id": new_key["keyhash"],
            }
        ]
        config_json["validator_key_ring"] = {
            new_key["keyhash"]: new_key["private"],
        }

        old_global_config_name = config_json["ton_global_config_name"]
        if old_global_config_name.startswith("/"):
            config_json["ton_global_config_name"] = str(
                Path(old_global_config_name).parent / mirrornet_global_config_name
            )
        else:
            config_json["ton_global_config_name"] = mirrornet_global_config_name
            old_global_config_name = str(Path(node["node_configs_path"]) / old_global_config_name)

        if "new_port" in node:
            endpoint = config_json["adnl_node"]["ip_address"]
            new_endpoint = f"{endpoint.split(':')[0]}:{node['new_port']}"
            config_json["adnl_node"]["ip_address"] = new_endpoint

        # upload config back
        new_config_str = json.dumps(config_json, indent=2)
        with open("config.json", "w") as f:
            f.write(new_config_str)
        cmd = f"scp -P {node['ssh_port']} config.json {node['username']}@{node['ip']}:{node['node_configs_path']}/config.json".split()
        run_command(cmd)
        os.remove("config.json")

        validator_configs.append(
            {
                "config": config_json,
                "new_key": new_key,
            }
        )
        print(" ✅ done")

    return validator_configs, old_global_config_name


def generate_hardfork_config(validator_configs: list[dict]) -> str:
    print("Generating hardfork config...", end="")
    new_config = {}
    now = int(time.time())
    new_config["p35"] = {}
    new_config["p35"]["utime_since"] = now - 24 * 3600
    new_config["p35"]["utime_until"] = now + 365 * 24 * 3600
    new_config["p35"]["total"] = len(validator_configs)
    new_config["p35"]["main"] = len(validator_configs)
    new_config["p35"]["total_weight"] = len(validator_configs) * 10
    validators = []
    for conf in validator_configs:
        pubkey = base64.b64decode(conf["new_key"]["public"]["pub_key"]).hex()
        validator_entry = {
            "public_key": pubkey,
            "weight": "10",
        }
        validators.append(validator_entry)
    new_config["p35"]["list"] = validators

    new_config["p37"] = new_config["p35"].copy()
    new_config["p37"]["utime_since"] = now + 365 * 24 * 3600
    new_config["p37"]["utime_until"] = now + 2 * 365 * 24 * 3600

    new_config["p33"] = new_config["p35"].copy()
    new_config["p33"]["utime_since"] = now - 365 * 24 * 3600
    new_config["p33"]["utime_until"] = now - 24 * 3600

    hardfork_config = json.dumps(new_config, indent=2)
    print(f" ✅ done, hardfork config: {hardfork_config}")
    return hardfork_config


def hardfork_filename(hardfork_info: dict) -> str:
    # transform base64 to hex string
    filename = base64.b64decode(hardfork_info["hardforks"][0]["root_hash"]).hex()
    return filename


def build_hardfork(config: str, node: dict, node_config: dict) -> dict:
    print("Building hardfork block...")

    # upload config to node
    print("  uploading hardfork config to node...", end="")
    with open("hardfork_config.json", "w") as f:
        f.write(config)
    cmd = f"scp -P {node['ssh_port']} hardfork_config.json {node['username']}@{node['ip']}:/tmp/hardfork_config.json".split()
    run_command(cmd)
    os.remove("hardfork_config.json")
    print(" ✅ done")

    db_path = node_config["internal_db_path"]

    # determine last masterchain block seqno
    print("  determining last masterchain block seqno...", end="")
    cmd = f"{node['hardfork_tool_path']} --last --path {db_path}"
    ssh = prepare_ssh_command(node, cmd)
    result = run_command(ssh)
    # exclude the last line
    result.stdout = "\n".join(result.stdout.splitlines()[:-1])
    last_block_info = json.loads(result.stdout)
    print(f" ✅ done, last masterchain block: {json.dumps(last_block_info, indent=2)}")

    hardfork_seqno = last_block_info["seqno"] - 10

    # run hardfork tool
    print(f"  running hardfork tool with seqno {hardfork_seqno}...", end="")
    cmd = f"{node['hardfork_tool_path']} --path {db_path} --state /tmp/hardfork_config.json --seqno {hardfork_seqno}"
    ssh = prepare_ssh_command(node, cmd)
    result = run_command(ssh)
    hardfork_block_info = json.loads(result.stdout)
    print(f" ✅ done, hardfork block info: {json.dumps(hardfork_block_info, indent=2)}")

    # hardfork block file name (transform base64 to hex string)
    print("  downloading hardfork block from node...", end="")
    filename = hardfork_filename(hardfork_block_info)
    cmd = f"scp -P {node['ssh_port']} {node['username']}@{node['ip']}:{filename} .".split()
    run_command(cmd)
    print(" ✅ done")

    return hardfork_block_info


def distribute_hardfork(hardfork_info: dict):
    # upload hardfork block to all nodes
    # it must be placed in configs directory and have name like <root_hash_hex> w/a extension
    print("Distributing hardfork block...")
    filename = hardfork_filename(hardfork_info)
    for node in config["nodes"]:
        print(f"  {node['ip']}...", end="")
        remote_path = f"{node['node_configs_path']}/{filename}"
        cmd = f"scp -P {node['ssh_port']} {filename} {node['username']}@{node['ip']}:{remote_path}".split()
        run_command(cmd)
        print(" ✅ done")
    os.remove(filename)


def build_global_config(
    hardfork_info: dict, validator_configs: list, old_global_config_name: str
) -> str:
    print("Building global config...")

    # download global config from node
    print("  downloading global config from node...", end="")
    cmd = f"cat {old_global_config_name}"
    ssh = prepare_ssh_command(config["nodes"][0], cmd)
    result = run_command(ssh)
    global_config = json.loads(result.stdout)
    print(" ✅ done")

    # build new dht nodes list uning our validators
    print("  Building DHT nodes list...", end="")
    nodes = []
    inode = 0
    for val_conf in validator_configs:
        node_conf = val_conf["config"]
        dht_key = None
        for key in node_conf["adnl_node"]["keys"]:
            if key["tag"] == 1:  # 1 is DHT tag
                dht_key = key["data"]["pvt_key"]
                break
        if dht_key is None:
            raise RuntimeError("DHT key not found in config")

        gendht_tool_path = Path(config["utils_path"]) / "gendht"
        ip = node_conf["adnl_node"]["ip_address"].split(":")[0]
        if ip == "0.0.0.0":
            ip = config["nodes"][inode]["ip"]
        inode += 1
        port = node_conf["adnl_node"]["ip_address"].split(":")[1]
        ip_address = f"{ip}:{port}"
        cmd = [str(gendht_tool_path), f"{ip_address}", dht_key]
        node = json.loads(run_command(cmd).stdout.strip())
        nodes.append(node)
    print(" ✅ done")

    # finally replace needed fields in global config
    global_config["dht"]["static_nodes"]["nodes"] = nodes
    global_config["liteservers"] = []
    if "hardforks" not in global_config["validator"]:
        global_config["validator"]["hardforks"] = []
    global_config["validator"]["hardforks"].append(hardfork_info["hardforks"][0])
    global_config["validator"]["init_block"] = hardfork_info["hardforks"][0]
    global_config_json = json.dumps(global_config, indent=2)

    print(f"  saving global config to {mirrornet_global_config_name}", end="")
    with open(mirrornet_global_config_name, "w") as f:
        f.write(global_config_json)
    print(" ✅ done")

    return global_config_json


def distribute_global_config(global_config: str, validator_configs: list[dict]):
    print("Distributing global config...")
    with open("global_config.json", "w") as f:
        f.write(global_config)
    for i in range(len(config["nodes"])):
        node = config["nodes"][i]
        node_config = validator_configs[i]["config"]
        print(f"  {node['ip']}...", end="")

        remote_path = node_config["ton_global_config_name"]
        if not remote_path.startswith("/"):
            remote_path = str(Path(node["node_bin_path"]).parent / remote_path)

        cmd = f"scp -P {node['ssh_port']} global_config.json {node['username']}@{node['ip']}:{remote_path}".split()
        run_command(cmd)
        print(" ✅ done")
    os.remove("global_config.json")


def run_nodes():
    print("Starting nodes...")
    for node in config["nodes"]:
        print(f"  {node['ip']}...", end="")
        dir_path = Path(node["node_bin_path"]).parent
        filename = Path(node["node_bin_path"]).name
        cmd = f"cd {dir_path}; nohup ./{filename} --configs {node['node_configs_path']} &"
        ssh = prepare_ssh_command(node, cmd)
        subprocess.Popen(ssh)
        print(" ✅ done")


def main():

    # Init script config
    if not load_config():
        return

    # start_time = time.time()

    stop_nodes()
    validator_configs, old_global_config_name = update_node_configs()
    hardfork_config = generate_hardfork_config(validator_configs)
    hardfork_info = build_hardfork(
        hardfork_config, config["nodes"][0], validator_configs[0]["config"]
    )
    distribute_hardfork(hardfork_info)
    global_config = build_global_config(
        hardfork_info, validator_configs, old_global_config_name
    )
    distribute_global_config(global_config, validator_configs)
    run_nodes()


if __name__ == "__main__":
    main()
