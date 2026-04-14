#!/usr/bin/env python3
"""
One-button bootstrap for local singlehost TON network + nodectl service.
Python equivalent of run_singlehost_nodectl.sh

Phases:
  1.  Build nodectl (skip with NOBUILD=1)
  2.  Generate nodectl config + create shared control-client key
  3.  Start singlehost network (--elections --control-client-public-key <key>),
      stop, restart all nodes once
  4.  Wait for blockchain progress
  5.  Complete nodectl config via CLI (import per-node keys, wallets, nodes,
      tick intervals, pools, bindings, enable elections)
  6.  Top up master wallet
  7.  Start nodectl service in background
  8.  Wait for validator wallets/pools to open, top them up
  9.  Wait for election participants
  10. Validate REST API: compare nodectl stake data with on-chain elector data
  11. Summary and exit assertions

Required env var:
  MASTER_WALLET_KEY  — 64-byte hex private key of the funded zerostate faucet wallet
                       (or place it in node/tests/test_load_net/.env)

Optional env vars:
  HTTP_API_URL, NODE_CNT, MASTER_TOPUP_TON, WALLET_TOPUP_TON, POOL_TOPUP_TON,
  PARTICIPANTS_WAIT_SECONDS, NOBUILD, KEEP_NODECTL_ON_SUCCESS, NODECTL_LOG, SCRIPT_LOG,
  TONCORE_VALIDATOR_SHARE / TONCORE_VALIDATOR_SHARE_ODD (basis points per TONCore slot; odd defaults to share)
  TONCORE_MIN_VALIDATOR_STAKE_TON / TONCORE_MIN_VALIDATOR_STAKE_ODD_TON (must differ so two derived pools
    are distinct; defaults 100000 / 100001)
  TONCORE_VALIDATOR_DEPOSIT_TON (per-slot deposit-validator amount in TON; default 100100,
    must be >= max(slot min stakes); auto-raises MASTER_TOPUP_TON if needed)
"""

from __future__ import annotations

import base64
import dataclasses
import datetime
import json
import os
import re
import secrets
import shutil
import signal
import subprocess
import sys
import time
import urllib.request
from pathlib import Path
from typing import Optional

# ── Constants ──────────────────────────────────────────────────────────────────
ELECTOR_ADDR   = "-1:3333333333333333333333333333333333333333333333333333333333333333"
TOTAL_PHASES   = 11
WALLET_VERSIONS = ["V1R3", "V3R2", "V4R2", "V5R1", "V3R2", "V3R2"]
# min_validator_stake default (100k TON) + 100 TON margin; matches app_config.rs
DEFAULT_TONCORE_DEPOSIT_TON = 100_100


# ══════════════════════════════════════════════════════════════════════════════
# Configuration & paths
# ══════════════════════════════════════════════════════════════════════════════

@dataclasses.dataclass
class Config:
    http_api_url:             str  = "http://127.0.0.1:3301"
    node_cnt:                 int  = 6
    master_topup:             str  = "1000"
    wallet_topup:             str  = "100"
    pool_topup:               str  = "100000"
    participants_wait:        int  = 600
    nobuild:                  bool = False
    keep_on_success:          bool = True
    toncore_validator_share:      int  = 5000   # basis points, slot 0 (even)
    toncore_validator_share_odd:  int  = 5000   # basis points, slot 1 (odd); env may override
    toncore_min_validator_stake_ton: int = 100_000   # slot 0 deploy param (TON)
    toncore_min_validator_stake_odd_ton: int = 100_001  # slot 1; must != slot 0 for two distinct pools
    toncore_validator_deposit_ton: int = DEFAULT_TONCORE_DEPOSIT_TON  # per-slot deposit-validator amount
    wallet_versions:              list = dataclasses.field(default_factory=lambda: list(WALLET_VERSIONS))

    @property
    def has_toncore(self) -> bool:
        """Last node gets a TONCore pool (two on-chain slots) when there are at least 2 nodes."""
        return self.node_cnt > 1

    @classmethod
    def from_env(cls) -> Config:
        return cls(
            http_api_url            = os.environ.get("HTTP_API_URL", "http://127.0.0.1:3301"),
            node_cnt                = int(os.environ.get("NODE_CNT", "6")),
            master_topup            = os.environ.get("MASTER_TOPUP_TON", "1000"),
            wallet_topup            = os.environ.get("WALLET_TOPUP_TON", "100"),
            pool_topup              = os.environ.get("POOL_TOPUP_TON", "100000"),
            participants_wait       = int(os.environ.get("PARTICIPANTS_WAIT_SECONDS", "600")),
            nobuild                 = os.environ.get("NOBUILD", "0") in ("1", "true"),
            keep_on_success         = os.environ.get("KEEP_NODECTL_ON_SUCCESS", "1") not in ("0", "false"),
            toncore_validator_share = int(os.environ.get("TONCORE_VALIDATOR_SHARE", "5000")),
            toncore_validator_share_odd = int(
                os.environ.get(
                    "TONCORE_VALIDATOR_SHARE_ODD",
                    os.environ.get("TONCORE_VALIDATOR_SHARE", "5000"),
                )
            ),
            toncore_min_validator_stake_ton = int(
                os.environ.get("TONCORE_MIN_VALIDATOR_STAKE_TON", "100000")
            ),
            toncore_min_validator_stake_odd_ton = int(
                os.environ.get("TONCORE_MIN_VALIDATOR_STAKE_ODD_TON", "100001")
            ),
            toncore_validator_deposit_ton = int(os.environ.get(
                "TONCORE_VALIDATOR_DEPOSIT_TON", str(DEFAULT_TONCORE_DEPOSIT_TON)
            )),
        )


@dataclasses.dataclass
class Paths:
    repo_root:       Path
    run_net_dir:     Path
    load_net_dir:    Path
    tmp_dir:         Path
    nodectl_src_bin: Path   # built binary at target/release/nodectl
    nodectl_bin:     Path   # working copy placed in tmp/ during phase 1
    nodectl_config:  Path
    vault_file:      Path
    nodectl_log:     Path
    script_log:      Path

    @classmethod
    def from_script_dir(cls, script_dir: Path) -> Paths:
        repo_root = script_dir.parents[2]   # …/test_run_net_py → src/
        tmp_dir   = script_dir / "tmp"
        return cls(
            repo_root       = repo_root,
            run_net_dir     = script_dir,
            load_net_dir    = repo_root / "node" / "tests" / "test_load_net",
            tmp_dir         = tmp_dir,
            nodectl_src_bin = repo_root / "target" / "release" / "nodectl",
            nodectl_bin     = tmp_dir / "nodectl",
            nodectl_config  = tmp_dir / "nodectl-config.json",
            vault_file      = script_dir / "vault.json",
            nodectl_log     = tmp_dir / _log_name("NODECTL_LOG", "nodectl-service.log"),
            script_log      = script_dir / _log_name("SCRIPT_LOG",  "singlehost-bootstrap.log"),
        )


def _log_name(env_key: str, default: str) -> str:
    """Return a normalised *.log filename from an env var."""
    name = Path(os.environ.get(env_key, default)).name
    return name if name.endswith(".log") else name + ".log"


# ══════════════════════════════════════════════════════════════════════════════
# Logger
# ══════════════════════════════════════════════════════════════════════════════

class Logger:
    """Writes coloured output to stdout and plain text to a log file."""

    _ANSI = re.compile(r"\033\[[0-9;]*m")

    def __init__(self, log_path: Path) -> None:
        log_path.parent.mkdir(parents=True, exist_ok=True)
        self._file = open(log_path, "w", buffering=1)

    def _emit(self, msg: str) -> None:
        print(msg, flush=True)
        self._file.write(self._ANSI.sub("", msg) + "\n")
        self._file.flush()

    def info(self,  msg: str) -> None: self._emit(f"\033[32m[INFO]\033[0m  {msg}")
    def warn(self,  msg: str) -> None: self._emit(f"\033[33m[WARN]\033[0m  {msg}")
    def error(self, msg: str) -> None: self._emit(f"\033[31m[ERROR]\033[0m {msg}")

    def close(self) -> None:
        self._file.close()


# ══════════════════════════════════════════════════════════════════════════════
# Bootstrap
# ══════════════════════════════════════════════════════════════════════════════

class BootstrapError(Exception):
    """Raised by phases on fatal errors; caught and logged in main()."""


class Bootstrap:
    def __init__(self, cfg: Config, paths: Paths, log: Logger) -> None:
        self.cfg   = cfg
        self.paths = paths
        self.log   = log
        # Runtime state — populated during phase 7 (start service)
        self._proc:           Optional[subprocess.Popen] = None
        self._nodectl_log:    Optional[Path]             = None
        self._service_log_fh: Optional[object]           = None

    # ── Orchestration ─────────────────────────────────────────────────────────

    def run(self) -> None:
        pub_key = self.phase2_generate_config()
        self.phase3_start_network(pub_key)
        self.phase4_wait_progress()
        master_addr = self.phase5_complete_config()
        self._ensure_bun_deps()
        self.phase6_topup_master(master_addr)
        self.phase7_start_service()
        wallet_addrs, pool_addrs = self.phase8_wait_and_topup()
        last_count = self.phase9_wait_participants()
        if last_count > 0:
            self.phase10_validate_api()
        else:
            self.log.warn("Skipping API validation — no participants found")
        self.phase11_summary(master_addr, wallet_addrs, pool_addrs, last_count)

    def shutdown(self, *, force: bool = False) -> None:
        """Terminate the nodectl service and network nodes if needed.
        force=True ignores keep_on_success."""
        if self._proc and self._proc.poll() is None:
            if force or not self.cfg.keep_on_success:
                self.log.info(f"Stopping nodectl (pid {self._proc.pid})")
                self._proc.terminate()
                try:
                    self._proc.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    self._proc.kill()
                    self._proc.wait()
        if self._service_log_fh:
            self._service_log_fh.close()
            self._service_log_fh = None
        if force:
            self._stop_network()

    def _stop_network(self) -> None:
        """Stop singlehost network nodes via test_run_net.py --stop."""
        try:
            py = self.paths.run_net_dir / ".venv" / "bin" / "python3"
            if not py.exists():
                py = Path(sys.executable)
            subprocess.run(
                [str(py), "test_run_net.py", "--stop"],
                cwd=self.paths.run_net_dir, check=False, timeout=15,
                stdin=subprocess.DEVNULL,
            )
        except Exception:
            pass

    # ── Internal helpers ──────────────────────────────────────────────────────

    def _phase(self, n: int, title: str) -> None:
        self.log.info(f"[{n}/{TOTAL_PHASES}] {title}")

    def _fail(self, msg: str) -> None:
        """Log an error and raise BootstrapError."""
        self.log.error(msg)
        raise BootstrapError(msg)

    def _nctl(self, *args: str, timeout: int = 30) -> None:
        """Run nodectl and let its output stream to the terminal."""
        result = subprocess.run(
            [str(self.paths.nodectl_bin), *args],
            capture_output=True, text=True,
            stdin=subprocess.DEVNULL, timeout=timeout,
        )
        if result.stdout:
            print(result.stdout, end="")
        if result.returncode != 0:
            self._fail(
                f"nodectl {' '.join(args)} failed (exit {result.returncode})"
                + (f": {result.stderr.strip()}" if result.stderr.strip() else "")
            )

    def _nctl_output(self, *args: str, check: bool = True, timeout: int = 30) -> str:
        """Run nodectl and return captured stdout."""
        result = subprocess.run(
            [str(self.paths.nodectl_bin), *args],
            capture_output=True, text=True, check=check,
            stdin=subprocess.DEVNULL, timeout=timeout,
        )
        return result.stdout

    def _json_rpc(self, method: str, params: Optional[dict] = None) -> dict:
        url     = self.cfg.http_api_url.rstrip("/") + "/jsonRPC"
        payload = json.dumps({"id": "1", "jsonrpc": "2.0",
                              "method": method, "params": params or {}}).encode()
        req = urllib.request.Request(url, data=payload,
                                     headers={"Content-Type": "application/json"})
        with urllib.request.urlopen(req, timeout=10) as resp:
            return json.loads(resp.read())

    def _seqno(self) -> Optional[int]:
        try:
            return int(self._json_rpc("getMasterchainInfo")["result"]["last"]["seqno"])
        except Exception:
            return None

    def _participant_count(self) -> int:
        try:
            r = self._json_rpc("runGetMethod", {
                "address": ELECTOR_ADDR,
                "method":  "participant_list_extended",
                "stack":   [],
            })
            return len(r["result"]["stack"][4][1].get("elements", []))
        except Exception:
            return 0

    def _wait_log(self, pattern: str, timeout: int) -> bool:
        """Poll nodectl log for pattern. Returns False on timeout or service death."""
        for _ in range(timeout):
            if self._proc and self._proc.poll() is not None:
                self.log.error(f"nodectl died while waiting for: {pattern!r}")
                return False
            try:
                if pattern in self._nodectl_log.read_text():  # type: ignore[union-attr]
                    return True
            except Exception:
                pass
            time.sleep(1)
        return False

    def _log_tail(self, n: int = 40) -> str:
        try:
            return "\n".join(self._nodectl_log.read_text().splitlines()[-n:])  # type: ignore[union-attr]
        except Exception:
            return ""

    def _bun_topup(self, address: str, amount: str) -> None:
        subprocess.run(["bun", "run", "topup", address, amount],
                       cwd=self.paths.load_net_dir, check=True,
                       stdin=subprocess.DEVNULL, timeout=120)

    def _address_balance_nanotons(self, address: str) -> Optional[int]:
        try:
            return int(self._json_rpc("getAddressInformation", {"address": address})["result"]["balance"])
        except Exception:
            return None

    def _wait_balance(self, address: str, min_nanotons: int, label: str, timeout: int = 120) -> None:
        """Poll until account balance >= min_nanotons; fail with context on timeout."""
        deadline = time.time() + timeout
        while time.time() < deadline:
            bal = self._address_balance_nanotons(address)
            if bal is not None and bal >= min_nanotons:
                return
            time.sleep(2)
        self._fail(f"{label}: {address} balance still below {min_nanotons / 1e9:.0f} TON after {timeout}s")

    def _toncore_wallet_topup_ton(self) -> int:
        """How much TON the validator wallet needs to cover two deposit-validator calls + gas."""
        return 2 * self.cfg.toncore_validator_deposit_ton + 50

    def _toncore_deposit_validator(self, log_text: str) -> None:
        """Fund validator wallet, then deposit-validator even + odd for last node's TONCore pool."""
        n = self.cfg.node_cnt
        binding = f"node{n}"
        dep = self.cfg.toncore_validator_deposit_ton
        m = re.search(rf"\[node{n}\] opened wallet: address=(\S+)", log_text)
        if not m:
            self._fail(f"Could not find validator wallet address for {binding} in nodectl log")
        waddr = m.group(1)

        wallet_ton = self._toncore_wallet_topup_ton()
        need_one_nano = (dep + 2) * 1_000_000_000  # deposit + gas
        self.log.info(f"  TONCore ({binding}): fund validator wallet ({wallet_ton} TON), deposit {dep} TON/slot")
        self._bun_topup(waddr, str(wallet_ton))
        self._wait_balance(waddr, need_one_nano, "Validator wallet not funded")

        for slot in ("even", "odd"):
            self.log.info(f"  TONCore deposit-validator --pool-{slot} ({dep} TON)")
            self._nctl("config", "pool", "deposit-validator",
                       "-b", binding, "-a", str(dep), f"--pool-{slot}", "--yes", timeout=120)
            time.sleep(8)  # wait for masterchain block confirmation before next deposit

    def _node_console(self, i: int) -> dict:
        path = self.paths.tmp_dir / f"node_{i}" / "console.json"
        return json.loads(path.read_text())

    def _venv_python(self) -> str:
        """Return path to venv's python3, creating the venv if needed."""
        venv_py = self.paths.run_net_dir / ".venv" / "bin" / "python3"
        if not venv_py.exists():
            subprocess.run([sys.executable, "-m", "venv",
                            str(self.paths.run_net_dir / ".venv")], check=True,
                           stdin=subprocess.DEVNULL, timeout=30)
            subprocess.run([str(venv_py), "-m", "pip", "install", "-q", "pyyaml"], check=True,
                           stdin=subprocess.DEVNULL, timeout=60)
        return str(venv_py)

    # ── Phase 1: Build ────────────────────────────────────────────────────────

    def phase1_build(self) -> None:
        self._phase(1, "Building nodectl...")
        if self.cfg.nobuild:
            self.log.info("  NOBUILD set, skipping build")
            if not self.paths.nodectl_src_bin.exists():
                self._fail(f"NOBUILD=1 but binary not found: {self.paths.nodectl_src_bin}")
        else:
            subprocess.run(["cargo", "build", "--release", "-p", "nodectl"],
                           cwd=self.paths.repo_root, check=True,
                           stdin=subprocess.DEVNULL)
        # Copy to tmp/ so all invocations run from a self-contained working directory
        shutil.copy2(self.paths.nodectl_src_bin, self.paths.nodectl_bin)
        self.log.info(f"  Copied binary → {self.paths.nodectl_bin}")
        ver = subprocess.run([str(self.paths.nodectl_bin), "--version"],
                             capture_output=True, text=True,
                             stdin=subprocess.DEVNULL, timeout=10)

        self.log.info(f"  {(ver.stdout or ver.stderr).strip() or 'version unknown'}")
    # ── Phase 2: Generate config ──────────────────────────────────────────────

    def phase2_generate_config(self) -> str:
        """Generate nodectl config and create shared control-client key.
        Returns the base64 public key of that key."""
        self._phase(2, "Pre-generating nodectl config and shared control-client key...")

        self.paths.nodectl_config.unlink(missing_ok=True)
        self.paths.vault_file.unlink(missing_ok=True)

        self.log.info("  config generate...")
        self._nctl("config", "generate", "--output", str(self.paths.nodectl_config), "--force")
        self.log.info("  config ton-http-api set...")
        self._nctl("config", "ton-http-api", "set", "--url", self.cfg.http_api_url)

        # Patch global tick_interval — no CLI command exists for this field
        cfg_json = json.loads(self.paths.nodectl_config.read_text())
        cfg_json["tick_interval"] = 20
        self.paths.nodectl_config.write_text(json.dumps(cfg_json, indent=2))
        self.log.info("  global tick_interval → 20")

        # Create the key used by nodes 3+ (nodes 1-2 get per-node keys in phase 5)
        self.log.info("  key add control-client-secret...")
        self._nctl("key", "add", "-n", "control-client-secret", "-e")

        # Extract its public key from the `key ls` tabular output
        for line in self._nctl_output("key", "ls").splitlines():
            parts = line.split()
            if parts and parts[0] == "control-client-secret":
                self.log.info(f"  shared control-client pub key: {parts[-1]}")
                return parts[-1]

        self._fail("Failed to extract pub key for control-client-secret")

    # ── Phase 3: Start network ────────────────────────────────────────────────

    def _ensure_test_run_net_config(self) -> None:
        """Generate test_run_net.json with correct node counts if it doesn't exist."""
        cfg_path = self.paths.run_net_dir / "test_run_net.json"
        if cfg_path.exists():
            cfg = json.loads(cfg_path.read_text())
            if cfg.get("rust_nodes_count") == self.cfg.node_cnt and cfg.get("cpp_nodes_count") == 0:
                return
            self.log.info(f"  Updating test_run_net.json: rust={self.cfg.node_cnt}, cpp=0")
        else:
            # Run test_run_net.py once to generate defaults, then patch
            py = self._venv_python()
            subprocess.run([py, "test_run_net.py"], cwd=self.paths.run_net_dir,
                           check=False, stdin=subprocess.DEVNULL, timeout=30)
            self.log.info(f"  Generated test_run_net.json: rust={self.cfg.node_cnt}, cpp=0")

        cfg = json.loads(cfg_path.read_text())
        cfg["rust_nodes_count"] = self.cfg.node_cnt
        cfg["cpp_nodes_count"] = 0
        cfg_path.write_text(json.dumps(cfg, indent=2))

    def phase3_start_network(self, pub_key_shared: str) -> None:
        """Start the singlehost network with the shared key pre-injected into every
        node's control_server.clients.list so no second restart is needed."""
        self._phase(3, "Starting singlehost network (--elections)...")
        py  = self._venv_python()
        rnd = self.paths.run_net_dir

        self._ensure_test_run_net_config()

        subprocess.run([py, "test_run_net.py", "--stop"], cwd=rnd, check=False,
                       stdin=subprocess.DEVNULL, timeout=30)

        net_args = ["--elections", "--control-client-public-key", pub_key_shared]
        if self.cfg.nobuild:
            net_args.append("--nobuild")
        subprocess.run([py, "test_run_net.py"] + net_args, cwd=rnd, check=True,
                       stdin=subprocess.DEVNULL,
                       env={**os.environ, "PYTHONUNBUFFERED": "1"})
        time.sleep(5)

    # ── Phase 4: Wait for progress ────────────────────────────────────────────

    def phase4_wait_progress(self) -> None:
        self._phase(4, "Waiting for blockchain progress...")
        seq_a = None
        for _ in range(60):
            seq_a = self._seqno()
            if seq_a is not None:
                break
            time.sleep(2)
        if seq_a is None:
            self._fail(f"Failed to read masterchain seqno from {self.cfg.http_api_url}")

        time.sleep(8)
        seq_b = self._seqno()
        if seq_b is None or seq_b <= seq_a:
            self._fail(f"Masterchain seqno not growing ({seq_a} → {seq_b})")
        self.log.info(f"  seqno: {seq_a} → {seq_b}")

    # ── Phase 5: Complete config ──────────────────────────────────────────────

    def phase5_complete_config(self) -> str:
        """Complete nodectl config via CLI. Returns the master wallet address."""
        self._phase(5, "Completing nodectl config via CLI...")

        self._add_keys()
        self._add_wallets()
        self._wait_http_api()
        master_addr = self._resolve_master_wallet()
        self._add_nodes()
        self._configure_elections(master_addr)

        return master_addr

    def _add_keys(self) -> None:
        self.log.info("  Creating remaining keys...")
        self._nctl("key", "add", "-n", "master-wallet-secret")
        for i in range(1, self.cfg.node_cnt + 1):
            self._nctl("key", "add", "-n", f"wallet{i}-secret")
        # Nodes 1-2 use their own per-node keys (imported from console.json)
        for i in range(1, min(3, self.cfg.node_cnt + 1)):
            pvt = self._node_console(i)["config"]["client_key"]["pvt_key"]
            self._nctl("key", "import", "-n", f"control-client-secret-{i}", "-e", "-k", pvt)
        self._nctl("key", "ls")

    def _add_wallets(self) -> None:
        self.log.info("  Adding wallets (different versions to exercise all wallet types)...")
        for i in range(1, self.cfg.node_cnt + 1):
            version = self.cfg.wallet_versions[i - 1] if i - 1 < len(self.cfg.wallet_versions) else "V3R2"
            self._nctl("config", "wallet", "add",
                       "-n", f"wallet{i}", "-s", f"wallet{i}-secret", "-v", version)
            self.log.info(f"    wallet{i} → {version}")

    def _wait_http_api(self, timeout: int = 120) -> None:
        self.log.info(f"  Waiting for HTTP API ({self.cfg.http_api_url}, timeout {timeout}s)...")
        deadline = time.time() + timeout
        while time.time() < deadline:
            try:
                urllib.request.urlopen(self.cfg.http_api_url, timeout=2)
                self.log.info("  HTTP API available")
                return
            except Exception:
                time.sleep(2)
        self._fail(f"HTTP API not available after {timeout}s")

    def _resolve_master_wallet(self) -> str:
        self.log.info("  Resolving master wallet address...")
        for _ in range(30):
            out = self._nctl_output("config", "master-wallet", "info", "--format=json", check=False)
            try:
                addr = json.loads(out).get("address") or ""
                if addr and addr not in ("unknown", "null"):
                    self.log.info(f"  Master wallet: {addr}")
                    return addr
            except Exception:
                pass
            time.sleep(3)
        self._fail("Could not resolve master wallet address")

    def _add_nodes(self) -> None:
        self.log.info("  Adding nodes...")
        for i in range(1, self.cfg.node_cnt + 1):
            console = self._node_console(i)
            # Nodes 1-2 have their own per-node keys; nodes 3+ use the shared key
            secret = f"control-client-secret-{i}" if i <= 2 else "control-client-secret"
            self._nctl("config", "node", "add",
                       "-n", f"node{i}",
                       "-e", console["config"]["server_address"],
                       "-p", console["config"]["server_key"]["pub_key"],
                       "-s", secret)
        self._nctl("config", "node", "ls")

    def _configure_elections(self, master_addr: str) -> None:
        self.log.info("  Setting elections tick interval → 20")
        self._nctl("config", "elections", "tick-interval", "20")

        self.log.info(
            "  Adding pools (SNP for node1..n-1, TONCore nominator on last node when n>1)..."
        )
        for i in range(1, self.cfg.node_cnt + 1):
            toncore_last = self.cfg.has_toncore and i == self.cfg.node_cnt
            if toncore_last:
                # add core: one command per slot with explicit slot selector.
                self._nctl(
                    "config",
                    "pool",
                    "add",
                    "core",
                    "-n",
                    f"pool{i}",
                    "--validator-share",
                    str(self.cfg.toncore_validator_share),
                    "--min-validator-stake",
                    str(self.cfg.toncore_min_validator_stake_ton),
                    "--even",
                )
                self._nctl(
                    "config",
                    "pool",
                    "add",
                    "core",
                    "-n",
                    f"pool{i}",
                    "--validator-share",
                    str(self.cfg.toncore_validator_share_odd),
                    "--min-validator-stake",
                    str(self.cfg.toncore_min_validator_stake_odd_ton),
                    "--odd",
                )
            else:
                self._nctl("config", "pool", "add", "-n", f"pool{i}", "-o", master_addr)


        time.sleep(10)  # let config settle before listing
        self._nctl("config", "pool", "ls")

        self.log.info("  Adding bindings...")
        for i in range(1, self.cfg.node_cnt + 1):
            self._nctl("config", "bind", "add",
                       "-n", f"node{i}", "-w", f"wallet{i}", "-p", f"pool{i}")

        self.log.info("  Enabling elections...")
        self._nctl("config", "elections", "enable",
                   *[f"node{i}" for i in range(1, self.cfg.node_cnt + 1)])
        self._nctl("config", "bind", "ls")

    # ── Phase 6: Top up master wallet ─────────────────────────────────────────

    def _minimum_master_topup_ton(self) -> int:
        """Minimum TON on master to cover TONCore deposits + all pool top-ups + cushion."""
        n = self.cfg.node_cnt
        pool_top = int(float(self.cfg.pool_topup))
        if not self.cfg.has_toncore:
            return n * pool_top + 500
        n_pool_addrs = n + 1  # (n-1) SNP + 2 TONCore contracts
        return self._toncore_wallet_topup_ton() + n_pool_addrs * pool_top + 500

    def phase6_topup_master(self, master_addr: str) -> None:
        floor_ton = int(float(self.cfg.master_topup))
        need_ton = self._minimum_master_topup_ton()
        planned = max(floor_ton, need_ton)
        if planned > floor_ton:
            self.log.info(
                f"  Master top-up raised to {planned} TON (env floor {floor_ton}; "
                f"auto minimum for pools + TONCore validator deposits {need_ton} TON)"
            )
        self._phase(6, f"Topping up master wallet ({planned} TON)...")
        self._bun_topup(master_addr, str(planned))

    # ── Phase 7: Start nodectl service ────────────────────────────────────────

    def phase7_start_service(self) -> None:
        self._phase(7, "Starting nodectl service...")
        self._nodectl_log = self.paths.nodectl_log
        self._service_log_fh = open(self._nodectl_log, "w")  # truncates previous run
        self._proc = subprocess.Popen(
            [str(self.paths.nodectl_bin), "service",
             "--config", str(self.paths.nodectl_config)],
            stdout=self._service_log_fh,
            stderr=subprocess.STDOUT,
            stdin=subprocess.DEVNULL,
            env={**os.environ, "RUST_LOG": "info"},
        )
        time.sleep(2)
        if self._proc.poll() is not None:
            self.log.error("nodectl service failed to start; last log lines:")
            print(self._log_tail(120), file=sys.stderr)
            raise BootstrapError("nodectl service failed to start")
        self.log.info(f"  nodectl service running (pid {self._proc.pid})")
        self.log.info(f"  log: {self._nodectl_log}")

    # ── Phase 8: Wait for wallets/pools, top them up ──────────────────────────

    def phase8_wait_and_topup(self) -> tuple:
        """Returns (wallet_addrs, pool_addrs) after opening and topping up."""
        self._phase(8, "Waiting for master wallet to open (up to 90s)...")
        if not self._wait_log("master wallet opened: address=", 90):
            self.log.error("No 'master wallet opened' after 90s")
            print(self._log_tail(120), file=sys.stderr)
            raise BootstrapError("master wallet did not open")

        self.log.info("  Waiting for validator wallets to open (up to 180s)...")
        if not self._wait_log("opened wallet: address=", 180):
            self.log.warn("No 'opened wallet' in log yet; continuing")

        self.log.info("  Waiting for nominator pools to open (up to 300s)...")
        # runtime_config: `[node] opened nominator pool(s): addr` (comma-separated for TONCore two slots).
        # Older builds: `opened nominator pool: address=…`
        if not self._wait_log("opened nominator pool", 300):
            self.log.warn("No 'opened nominator pool' in log yet; continuing")

        self.log.info("  Waiting for all contracts to be deployed (up to 300s)...")
        if not self._wait_log("all contracts are ready", 300):
            self._fail("Contracts not ready after 300s")

        log_text     = self._nodectl_log.read_text()  # type: ignore[union-attr]
        wallet_addrs = sorted(set(re.findall(r"opened wallet: address=(\S+)", log_text)))
        pool_addrs_set: set[str] = set(re.findall(r"opened nominator pool: address=(\S+)", log_text))
        for m in re.finditer(r"opened nominator pool\(s\): (.+)", log_text):
            for part in m.group(1).split(","):
                a = part.strip()
                if a:
                    pool_addrs_set.add(a)
        pool_addrs = sorted(pool_addrs_set)
        self.log.info(f"  Wallets opened: {len(wallet_addrs)}, pools opened: {len(pool_addrs)}")

        if self.cfg.has_toncore:
            self._toncore_deposit_validator(log_text)

        for addr in pool_addrs:
            self.log.info(f"  Top up pool   {addr} ({self.cfg.pool_topup} TON)")
            self._bun_topup(addr, self.cfg.pool_topup)
            time.sleep(5) # wait for the pool to be topped up
        self._nctl("config", "pool", "ls")

        return wallet_addrs, pool_addrs

    # ── Phase 9: Wait for election participants ────────────────────────────────

    def phase9_wait_participants(self) -> int:
        expected = self.cfg.node_cnt
        self._phase(9, f"Waiting for {expected} election participants (up to {self.cfg.participants_wait}s)...")
        deadline = time.time() + self.cfg.participants_wait
        while time.time() < deadline:
            cnt = self._participant_count()
            if cnt >= expected:
                return cnt
            self.log.info(f"  participants: {cnt}/{expected}")
            time.sleep(5)
        cnt = self._participant_count()
        if cnt < expected:
            self._fail(f"Expected {expected} participants but got {cnt} after {self.cfg.participants_wait}s")
        return cnt

    # ── Phase 10: Auth + REST API stake validation ──────────────────────────

    def phase10_validate_api(self) -> None:
        self._phase(10, "Setting up auth and validating REST API stakes...")

        # Create API user and obtain JWT token
        password = secrets.token_hex(16)
        result = subprocess.run(
            [str(self.paths.nodectl_bin), "auth", "add",
             "--username", "admin", "--role", "operator", "--password-stdin"],
            input=password, text=True, capture_output=True, timeout=15,
        )
        if result.returncode != 0:
            self._fail(f"auth add failed (exit {result.returncode}): {result.stderr.strip()}")
        self.log.info("  Created auth user 'admin' (operator)")

        # Service reloads config from disk every 10s — wait for it to pick up the new user
        time.sleep(12)

        result = subprocess.run(
            [str(self.paths.nodectl_bin), "api", "login", "admin", "--password-stdin"],
            input=password, capture_output=True, text=True, check=True, timeout=15,
        )
        os.environ["NODECTL_API_TOKEN"] = json.loads(result.stdout)["token"]
        self.log.info("  Logged in and exported NODECTL_API_TOKEN")

        elections = self._fetch_nodectl_elections()
        if elections is None:
            return

        elector_map = self._fetch_elector_stake_map()
        if elector_map is None:
            return

        self._compare_stakes(elections, elector_map)

    def _fetch_nodectl_elections(self) -> Optional[dict]:
        result = subprocess.run(
            [str(self.paths.nodectl_bin), "api", "elections", "--format=json"],
            capture_output=True, text=True,
            stdin=subprocess.DEVNULL, timeout=15,
        )
        stderr = result.stderr.strip()
        if result.returncode != 0:
            self.log.warn(f"  nodectl api elections failed (exit {result.returncode})"
                          + (f": {stderr}" if stderr else ""))
            return None
        try:
            return json.loads(result.stdout)
        except Exception as e:
            self.log.warn(f"  Could not parse elections response: {e}; skipping")
            return None

    def _fetch_elector_stake_map(self) -> Optional[dict]:
        """Returns {pubkey_bytes: stake_str} for every current elector participant."""
        try:
            resp = self._json_rpc("runGetMethod", {
                "address": ELECTOR_ADDR,
                "method":  "participant_list_extended",
                "stack":   [],
            })
        except Exception as e:
            self.log.warn(f"  Could not fetch participant_list_extended: {e}; skipping")
            return None

        result = {}
        try:
            # Each element is a StackEntryJson dict: {"@type": "tvm.stackEntry*", ...}
            # Numbers: {"@type": "tvm.stackEntryNumber", "number": {"@type": "tvm.numberDecimal", "number": "<decimal>"}}
            # Tuples:  {"@type": "tvm.stackEntryTuple",  "tuple":  {"@type": "tvm.tuple", "elements": [...]}}
            for entry in resp["result"]["stack"][4][1].get("elements", []):
                inner      = entry["tuple"]["elements"]
                pubkey_str = inner[0]["number"]["number"]
                stake_str  = inner[1]["tuple"]["elements"][0]["number"]["number"]
                n = int(pubkey_str, 16) if pubkey_str.lower().startswith("0x") else int(pubkey_str)
                result[n.to_bytes(32, "big")] = stake_str
        except (KeyError, IndexError, TypeError, ValueError) as e:
            self.log.warn(f"  Could not parse elector participant list: {type(e).__name__}: {e}; skipping")
            return None

        return result

    def _compare_stakes(self, elections: dict, elector_map: dict) -> None:
        mismatches, accepted = 0, 0
        for p in elections.get("our_participants", []):
            if not p.get("stake_accepted"):
                continue
            pubkey_b64    = p.get("pubkey")
            accepted_stake = p.get("accepted_stake")
            if not pubkey_b64 or not accepted_stake:
                continue
            accepted += 1
            key_bytes     = base64.b64decode(pubkey_b64)
            elector_stake = elector_map.get(bytes(key_bytes))
            node_id       = p.get("node_id", "?")
            if elector_stake is None:
                self.log.warn(f"  [MISMATCH] {node_id}: pubkey not found in elector list")
                mismatches += 1
            elif elector_stake != accepted_stake:
                self.log.warn(
                    f"  [MISMATCH] {node_id}: "
                    f"nodectl={accepted_stake} != elector={elector_stake} nanotons"
                )
                mismatches += 1
            else:
                self.log.info(f"  [OK] {node_id}: accepted_stake={accepted_stake} nanotons")

        self.log.info(f"  Participants with accepted stake: {accepted}, mismatches: {mismatches}")
        if accepted == 0:
            self.log.warn("  No accepted stakes in nodectl API response; skipping comparison")
            return
        if mismatches:
            self._fail("Stake mismatch between nodectl REST API and elector contract")
        self.log.info("  REST API stake comparison: OK")

    # ── Phase 11: Summary ─────────────────────────────────────────────────────

    def phase11_summary(
        self, master_addr: str, wallet_addrs: list, pool_addrs: list, last_count: int
    ) -> None:
        self._phase(11, "Summary")
        rows = [
            ("nodectl pid",    str(self._proc.pid) if self._proc else "N/A"),
            ("nodectl log",    str(self._nodectl_log)),
            ("master wallet",  master_addr),
            ("opened wallets", str(len(wallet_addrs))),
            ("opened pools",   str(len(pool_addrs))),
            ("participants",   str(last_count)),
        ]
        for key, val in rows:
            print(f"  {key + ':':<18} {val}")

        if last_count == 0:
            self._fail(f"No election participants found after {self.cfg.participants_wait}s")
        self.log.info(f"  elections: OK ({last_count} participant(s))")

        print()
        self.log.info("Bootstrap complete. nodectl service running in background.")
        if self._proc:
            print(f"Stop command: kill {self._proc.pid}")

    # ── Misc helpers ──────────────────────────────────────────────────────────

    def _ensure_bun_deps(self) -> None:
        if not (self.paths.load_net_dir / "node_modules").exists():
            subprocess.run(["bun", "install", "--silent"], cwd=self.paths.load_net_dir, check=True,
                           stdin=subprocess.DEVNULL, timeout=60)

# ══════════════════════════════════════════════════════════════════════════════
# Entry point
# ══════════════════════════════════════════════════════════════════════════════

def main() -> None:
    try:
        paths = Paths.from_script_dir(Path(__file__).resolve().parent)
        cfg   = Config.from_env()
        paths.tmp_dir.mkdir(parents=True, exist_ok=True)
        log = Logger(paths.script_log)
    except Exception as e:
        print(f"\033[31m[FATAL]\033[0m Failed during early init: {e}", file=sys.stderr)
        sys.exit(1)

    ts = datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%d %H:%M:%S UTC")
    log.info(f"=== {ts} run_singlehost_nodectl.py started ===")
    log.info(f"Script log: {paths.script_log}")

    bootstrap = Bootstrap(cfg, paths, log)

    # Signal handling — needs a bootstrap reference for clean shutdown
    def on_signal(_sig: int, _frame: object) -> None:
        bootstrap.shutdown(force=True)
        log.close()
        sys.exit(130)

    signal.signal(signal.SIGINT, on_signal)
    signal.signal(signal.SIGTERM, on_signal)

    # Preflight checks
    for cmd in ("cargo", "bun", "curl", "openssl"):
        if not shutil.which(cmd):
            log.error(f"Missing required command: {cmd}")
            sys.exit(1)

    if not os.environ.get("MASTER_WALLET_KEY"):
        log.error("MASTER_WALLET_KEY is not set.")
        log.error(f"Set it in the environment or add it to {paths.load_net_dir / '.env'}")
        sys.exit(1)

    os.environ["API_ENDPOINTS"] = cfg.http_api_url.rstrip("/") + "/"
    os.environ["VAULT_URL"] = f"file://vault.json?master_key={secrets.token_hex(32)}"
    log.info(f"VAULT_URL={os.environ['VAULT_URL']}")

    # All nodectl CLI invocations discover the config via this env var
    os.environ["CONFIG_PATH"] = str(paths.nodectl_config)

    # Run all phases; BootstrapError is our structured failure signal
    exit_code = 0
    try:
        bootstrap.phase1_build()
        bootstrap.run()
    except BootstrapError:
        exit_code = 1   # error already logged inside _fail()
    except Exception:
        import traceback
        log.error(f"Unexpected error:\n{traceback.format_exc()}")
        exit_code = 1
    finally:
        bootstrap.shutdown(force=(exit_code != 0))
        log.close()

    sys.exit(exit_code)


if __name__ == "__main__":
    main()
