#!/usr/bin/env python3
"""
One-button bootstrap for local singlehost TON + nodectl (extended scenario).

Python companion to run_singlehost_nodectl.py: same phases 1–7 and 9–10, different pool
topology and post-validation. Default logs use *-snp-toncore* filenames so runs do not
overwrite the base script's singlehost-bootstrap.log.

Scenario: 7 validators — 2× single-nominator (snp1, snp2) + 5× TONCore dual-slot pools
(core1..core5), stake policy split50, even/odd min_validator_stake differs by 1 TON.

Phases:
  1.  Build nodectl (skip with NOBUILD=1)
  2.  Generate nodectl config + shared control-client key
  3.  Start singlehost network (--elections, --control-client-public-key)
  4.  Wait for blockchain progress
  5.  Complete nodectl config (keys, wallets, nodes, SNP+TONCore pools, bindings, elections)
  6.  Top up master wallet; wait for chain + on-chain balance (avoids racing nodectl)
  7.  Start nodectl service in background
  8.  Wait for deploy → top up TONCore validator wallets → deposit-validator even/odd → pool topups
  9.  Wait for election participants (expects NODE_CNT, default 7)
  10. Validate REST API stakes vs on-chain elector data
  11. Observe several election_id transitions (split50); log per-node stake lines each poll
  12. Summary and exit assertions

Required env var:
  MASTER_WALLET_KEY  — 64-byte hex private key of the funded zerostate faucet wallet
                       (or place it in node/tests/test_load_net/.env)

Optional env vars:
  HTTP_API_URL, NODE_CNT (default 7), MASTER_TOPUP_TON, WALLET_TOPUP_TON, POOL_TOPUP_TON,
  CORE_VALIDATOR_DEPOSIT_TON (TONCore validator deposits; wallets pre-funded for 2× deposit + fees),
  OBSERVE_ROUNDS, OBSERVE_INTERVAL_SEC,
  PARTICIPANTS_WAIT_SECONDS, NOBUILD, KEEP_NODECTL_ON_SUCCESS, NODECTL_LOG, SCRIPT_LOG
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
import urllib.error
import urllib.request
from pathlib import Path
from typing import Optional

# ── Constants ──────────────────────────────────────────────────────────────────
ELECTOR_ADDR   = "-1:3333333333333333333333333333333333333333333333333333333333333333"
TOTAL_PHASES   = 12
# Seven validators: exercise several wallet versions + duplicates.
WALLET_VERSIONS = ["V1R3", "V3R2", "V4R2", "V5R1", "V3R2", "V3R2", "V4R2"]
SNP_POOL_NAMES  = ("snp1", "snp2")
CORE_POOL_NAMES = tuple(f"core{i}" for i in range(1, 6))  # core1 .. core5
SNP_NODE_COUNT   = len(SNP_POOL_NAMES)
CORE_NODE_OFFSET = SNP_NODE_COUNT + 1  # node3 .. node7 for TONCore
# TONCore dual slot: same validator_share; odd/even differ by min_validator_stake (+1 TON on odd).
TONCORE_VALIDATOR_SHARE_BP = 1000
TONCORE_MIN_VALIDATOR_STAKE_EVEN_TON = 10_000.0
TONCORE_MIN_VALIDATOR_STAKE_ODD_TON = 10_001.0


def _validate_toncore_min_stakes_and_deposit(cfg: "Config") -> None:
    """Fail fast when even/odd min stakes match or deposit is below the larger min stake."""
    ev = TONCORE_MIN_VALIDATOR_STAKE_EVEN_TON
    od = TONCORE_MIN_VALIDATOR_STAKE_ODD_TON
    if ev == od:
        raise ValueError(
            "TONCore min_validator_stake for even and odd slots must differ "
            f"(TONCORE_MIN_VALIDATOR_STAKE_EVEN_TON and _ODD_TON are both {ev}); "
            "equal values would not yield two distinct TONCore pool addresses."
        )
    need = max(ev, od)
    if cfg.core_validator_deposit < need:
        raise ValueError(
            f"CORE_VALIDATOR_DEPOSIT_TON ({cfg.core_validator_deposit}) must be >= "
            f"max(min_validator_stake even, odd) ({need} TON). "
            "Otherwise deposit-validator fails or leaves pools under their minimum."
        )


# ══════════════════════════════════════════════════════════════════════════════
# Configuration & paths
# ══════════════════════════════════════════════════════════════════════════════

@dataclasses.dataclass
class Config:
    http_api_url:           str    = "http://127.0.0.1:3301"
    node_cnt:               int    = SNP_NODE_COUNT + len(CORE_POOL_NAMES)
    master_topup:           str    = "1000"
    wallet_topup:           str    = "100"
    pool_topup:             str    = "100000"
    core_validator_deposit: float  = 50_000.0
    observe_rounds:         int    = 4
    observe_interval_sec:   int    = 20
    participants_wait:      int    = 600
    nobuild:                bool  = False
    keep_on_success:        bool  = True
    wallet_versions:        list  = dataclasses.field(default_factory=lambda: list(WALLET_VERSIONS))

    @classmethod
    def from_env(cls) -> Config:
        cfg = cls(
            http_api_url      = os.environ.get("HTTP_API_URL", "http://127.0.0.1:3301"),
            node_cnt          = int(os.environ.get("NODE_CNT", str(SNP_NODE_COUNT + len(CORE_POOL_NAMES)))),
            master_topup      = os.environ.get("MASTER_TOPUP_TON", "1000"),
            wallet_topup      = os.environ.get("WALLET_TOPUP_TON", "100"),
            pool_topup        = os.environ.get("POOL_TOPUP_TON", "100000"),
            core_validator_deposit=float(os.environ.get("CORE_VALIDATOR_DEPOSIT_TON", "50000")),
            observe_rounds         = int(os.environ.get("OBSERVE_ROUNDS", "4")),
            observe_interval_sec = int(os.environ.get("OBSERVE_INTERVAL_SEC", "20")),
            participants_wait = int(os.environ.get("PARTICIPANTS_WAIT_SECONDS", "600")),
            nobuild           = os.environ.get("NOBUILD", "0") in ("1", "true"),
            keep_on_success   = os.environ.get("KEEP_NODECTL_ON_SUCCESS", "1") not in ("0", "false"),
        )
        _validate_toncore_min_stakes_and_deposit(cfg)
        return cfg


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
            nodectl_log     = tmp_dir / _log_name("NODECTL_LOG", "nodectl-service-snp-toncore.log"),
            script_log      = script_dir / _log_name("SCRIPT_LOG", "singlehost-bootstrap-snp-toncore.log"),
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
        seq_before_topup = self._seqno()
        self.phase6_topup_master(master_addr)
        # Topup tx must be applied in a produced block before HTTP API shows the new balance.
        self._wait_chain_after_topup(seq_before_topup, max_wait=120)
        self._wait_master_wallet_funded(master_addr, min_ton=2.0, timeout=240)
        self.phase7_start_service()
        wallet_addrs, pool_addrs, _node_pool_map = self.phase8_wait_and_topup()
        last_count = self.phase9_wait_participants()
        if last_count > 0:
            self.phase10_validate_api()
            self.phase11_observe_election_rounds()
        else:
            self.log.warn("Skipping API validation — no participants found")
        self.phase12_summary(master_addr, wallet_addrs, pool_addrs, last_count)

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

    def _account_balance_nanotons(self, address: str) -> Optional[int]:
        """Return account balance in nanotons via ton-http-api jsonRPC, or None on failure."""
        addr = address.strip()
        try:
            r = self._json_rpc("getAddressInformation", {"address": addr})
            if r.get("ok") is False:
                return None
            res = r.get("result")
            if not isinstance(res, dict):
                return None
            b = res.get("balance")
            if b is None:
                return None
            s = str(b).strip().replace(" ", "").replace("_", "")
            return int(s, 10)
        except (TypeError, ValueError, KeyError, urllib.error.HTTPError, urllib.error.URLError):
            return None
        except Exception:
            return None

    def _wait_chain_after_topup(self, seq_before: Optional[int], max_wait: int) -> None:
        """Wait until masterchain moves forward so getAddressInformation sees the topup."""
        self.log.info(
            f"  Waiting for masterchain to advance after topup (baseline seqno={seq_before}, "
            f"up to {max_wait}s)..."
        )
        if seq_before is None:
            self.log.warn("  No baseline seqno; sleeping 15s so the topup can land in a block")
            time.sleep(15)
            return
        deadline = time.time() + max_wait
        while time.time() < deadline:
            seq = self._seqno()
            if seq is not None and seq > seq_before:
                self.log.info(f"  Masterchain seqno advanced {seq_before} → {seq}")
                time.sleep(3)
                return
            time.sleep(2)
        self.log.warn(
            f"  Seqno did not advance within {max_wait}s (current={self._seqno()}); "
            "sleeping 10s and continuing — balance check may still work"
        )
        time.sleep(10)

    def _wait_master_wallet_funded(self, master_addr: str, min_ton: float, timeout: int) -> None:
        """Wait until the chain reports enough TON on master (avoids nodectl racing the topup)."""
        need = int(min_ton * 1_000_000_000)
        self.log.info(
            f"  Waiting for master on-chain balance ≥ {min_ton:g} TON ({timeout}s)..."
        )
        last_bal: Optional[int] = None
        for i in range(timeout):
            bal = self._account_balance_nanotons(master_addr)
            last_bal = bal if bal is not None else last_bal
            if bal is not None and bal >= need:
                self.log.info(f"  Master funded: {bal / 1e9:.4f} TON")
                return
            time.sleep(1)
        hint = (
            f" last_balance_read={last_bal}"
            if last_bal is not None
            else " getAddressInformation returned no usable balance (check address format / jsonRPC)"
        )
        self._fail(
            f"Master wallet {master_addr} still below {min_ton:g} TON after {timeout}s.{hint}"
        )

    def _wait_wallet_onchain_balance(
        self, address: str, min_nanotons: int, timeout: int, label: str
    ) -> None:
        """Poll getAddressInformation until balance is enough (nanotons)."""
        need_ton = min_nanotons / 1e9
        self.log.info(
            f"  Waiting for {label} on-chain balance ≥ {need_ton:g} TON ({timeout}s)..."
        )
        last: Optional[int] = None
        for _ in range(timeout):
            bal = self._account_balance_nanotons(address)
            last = bal if bal is not None else last
            if bal is not None and bal >= min_nanotons:
                self.log.info(f"  {label} ready: {bal / 1e9:.4f} TON")
                return
            time.sleep(1)
        self._fail(
            f"{label} ({address}) still below {need_ton:g} TON on-chain after {timeout}s "
            f"(last read: {last})"
        )

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
        timeout_raw = os.environ.get("BUN_TOPUP_TIMEOUT_SECONDS", "120")
        try:
            timeout = int(timeout_raw)
        except ValueError:
            timeout = 120
        subprocess.run(["bun", "run", "topup", address, amount],
                       cwd=self.paths.load_net_dir, check=True,
                       stdin=subprocess.DEVNULL, timeout=timeout)

    def _wallet_address_from_config(self, wallet_name: str) -> str:
        """Resolve raw workchain address for a named wallet via nodectl JSON."""
        out = self._nctl_output("config", "wallet", "ls", "--format=json", timeout=60)
        try:
            rows = json.loads(out)
        except json.JSONDecodeError as e:
            self._fail(f"config wallet ls --format=json: {e}")
        for row in rows:
            if row.get("name") == wallet_name:
                addr = row.get("address")
                if isinstance(addr, str) and addr.strip():
                    return addr.strip()
        self._fail(f"No address in config for wallet {wallet_name!r}")

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
        expected = SNP_NODE_COUNT + len(CORE_POOL_NAMES)
        if self.cfg.node_cnt != expected:
            self._fail(
                f"This script expects NODE_CNT={expected} (2 SNP + 5 TONCore), got {self.cfg.node_cnt}"
            )

        self.log.info("  Setting elections stake policy → split50 (default for all nodes)")
        self._nctl("config", "elections", "stake-policy", "--split50")

        self.log.info("  Setting elections tick interval → 20")
        self._nctl("config", "elections", "tick-interval", "20")

        self.log.info(
            "  Adding pools (2 SNP + 5 TONCore dual-slot: odd vs even min_validator_stake +1 TON)..."
        )
        for name in SNP_POOL_NAMES:
            self._nctl("config", "pool", "add", "-n", name, "-o", master_addr)
        for name in CORE_POOL_NAMES:
            self._nctl(
                "config", "pool", "add", "core",
                "-n", name,
                "--even",
                "--validator-share", str(TONCORE_VALIDATOR_SHARE_BP),
                "--min-validator-stake", str(int(TONCORE_MIN_VALIDATOR_STAKE_EVEN_TON)),
            )
            self._nctl(
                "config", "pool", "add", "core",
                "-n", name,
                "--odd",
                "--validator-share", str(TONCORE_VALIDATOR_SHARE_BP),
                "--min-validator-stake", str(int(TONCORE_MIN_VALIDATOR_STAKE_ODD_TON)),
            )

        self.log.info("  Adding bindings (node1–2 → SNP, node3–7 → TONCore)...")
        self._nctl("config", "bind", "add", "-n", "node1", "-w", "wallet1", "-p", SNP_POOL_NAMES[0])
        self._nctl("config", "bind", "add", "-n", "node2", "-w", "wallet2", "-p", SNP_POOL_NAMES[1])
        for i, pname in enumerate(CORE_POOL_NAMES, start=CORE_NODE_OFFSET):
            self._nctl("config", "bind", "add", "-n", f"node{i}", "-w", f"wallet{i}", "-p", pname)

        self.log.info("  Enabling elections...")
        self._nctl("config", "elections", "enable",
                   *[f"node{i}" for i in range(1, self.cfg.node_cnt + 1)])
        self._nctl("config", "bind", "ls")

    # ── Phase 6: Top up master wallet ─────────────────────────────────────────

    def phase6_topup_master(self, master_addr: str) -> None:
        self._phase(6, f"Topping up master wallet ({self.cfg.master_topup} TON)...")
        self._bun_topup(master_addr, self.cfg.master_topup)

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

    @staticmethod
    def _parse_node_pool_map(log_text: str) -> dict[str, list[str]]:
        """Parse `[nodeN] opened nominator pool(s): addr1, addr2` lines (SNP + TONCore)."""
        out: dict[str, list[str]] = {}
        for line in log_text.splitlines():
            m = re.search(r"\[([^\]]+)\]\s+opened nominator pool\(s\):\s*(.*)$", line)
            if not m:
                continue
            node, rest = m.group(1).strip(), m.group(2).strip()
            addrs = [a.strip() for a in rest.split(",") if a.strip()]
            if addrs:
                out[node] = addrs
        return out

    def phase8_wait_and_topup(self) -> tuple:
        """Returns (wallet_addrs, pool_addrs, node_pool_map)."""
        self._phase(
            8,
            "Waiting for deploy, TONCore validator deposits (even/odd), then pool top-ups...",
        )
        if not self._wait_log("master wallet opened: address=", 90):
            self.log.error("No 'master wallet opened' after 90s")
            print(self._log_tail(120), file=sys.stderr)
            raise BootstrapError("master wallet did not open")

        self.log.info("  Waiting for validator wallets to open (up to 180s)...")
        if not self._wait_log("opened wallet: address=", 180):
            self.log.warn("No 'opened wallet' in log yet; continuing")

        self.log.info("  Waiting for nominator pools to open (up to 300s)...")
        if not self._wait_log("opened nominator pool(s):", 300):
            self.log.warn("No 'opened nominator pool' in log yet; continuing")

        self.log.info("  Waiting for all contracts to be deployed (up to 600s)...")
        if not self._wait_log("all contracts are ready", 600):
            self.log.error("Last nodectl log lines:")
            print(self._log_tail(80), file=sys.stderr)
            self._fail("Contracts not ready after 600s")

        log_text      = self._nodectl_log.read_text()  # type: ignore[union-attr]
        wallet_addrs  = sorted(set(re.findall(r"opened wallet: address=(\S+)", log_text)))
        node_pool_map = self._parse_node_pool_map(log_text)
        pool_addrs = sorted(
            {a for addrs in node_pool_map.values() for a in addrs}
        )
        self.log.info(
            f"  Wallets opened: {len(wallet_addrs)}, unique pool addresses: {len(pool_addrs)}"
        )
        if node_pool_map:
            for node in sorted(node_pool_map.keys(), key=lambda s: (len(s), s)):
                addrs = node_pool_map[node]
                self.log.info(f"    {node}: {len(addrs)} pool address(es) — {', '.join(addrs)}")

        # `deposit-validator` debits the bound validator wallet, not the master; TONCore needs
        # even+odd deposits plus fees — default ~10 TON from deploy is not enough.
        toncore_wallet_topup = int(2 * float(self.cfg.core_validator_deposit)) + 3000
        topup_amt = str(toncore_wallet_topup)
        self.log.info(
            f"  Topping up TONCore validator wallets wallet{CORE_NODE_OFFSET}..wallet{self.cfg.node_cnt} "
            f"({topup_amt} TON each) before deposit-validator..."
        )
        toncore_wallet_addrs: list[tuple[str, str]] = []
        for i in range(CORE_NODE_OFFSET, self.cfg.node_cnt + 1):
            wname = f"wallet{i}"
            waddr = self._wallet_address_from_config(wname)
            toncore_wallet_addrs.append((wname, waddr))
            self.log.info(f"  Top up {wname} {waddr} ({topup_amt} TON)")
            self._bun_topup(waddr, topup_amt)
            time.sleep(5)

        # Bun returns after send; balance on each wallet may lag several blocks behind node5's turn.
        min_nanotons = int((2 * float(self.cfg.core_validator_deposit) + 5.0) * 1_000_000_000)
        for wname, waddr in toncore_wallet_addrs:
            self._wait_wallet_onchain_balance(
                waddr,
                min_nanotons=min_nanotons,
                timeout=180,
                label=wname,
            )

        dep_s = f"{self.cfg.core_validator_deposit:g}"
        for i in range(CORE_NODE_OFFSET, self.cfg.node_cnt + 1):
            bname = f"node{i}"
            self.log.info(
                f"  TONCore validator deposit {dep_s} TON (even) → {bname} (--pool-even)..."
            )
            self._nctl(
                "config", "pool", "deposit-validator",
                "-b", bname, "-a", dep_s, "--yes", "--pool-even",
                timeout=180,
            )
            time.sleep(2)
            self.log.info(
                f"  TONCore validator deposit {dep_s} TON (odd) → {bname} (--pool-odd)..."
            )
            self._nctl(
                "config", "pool", "deposit-validator",
                "-b", bname, "-a", dep_s, "--yes", "--pool-odd",
                timeout=180,
            )
            time.sleep(2)

        for addr in pool_addrs:
            self.log.info(f"  Top up pool   {addr} ({self.cfg.pool_topup} TON)")
            self._bun_topup(addr, self.cfg.pool_topup)
            time.sleep(5)
        self._nctl("config", "pool", "ls")

        return wallet_addrs, pool_addrs, node_pool_map

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

    # ── Phase 11: Observe election rounds (split50) ─────────────────────────────

    def _fetch_nodectl_elections_json(self) -> Optional[dict]:
        tok = os.environ.get("NODECTL_API_TOKEN")
        if not tok:
            return None
        result = subprocess.run(
            [str(self.paths.nodectl_bin), "api", "elections", "--format=json"],
            capture_output=True, text=True,
            stdin=subprocess.DEVNULL, timeout=20,
            env={**os.environ, "NODECTL_API_TOKEN": tok},
        )
        if result.returncode != 0:
            return None
        try:
            return json.loads(result.stdout)
        except Exception:
            return None

    def phase11_observe_election_rounds(self) -> None:
        """Poll REST elections + pool table across several election_id changes (split50)."""
        self._phase(
            11,
            f"Observing stakes (~{self.cfg.observe_rounds} election transitions, "
            f"interval {self.cfg.observe_interval_sec}s, policy split50)...",
        )
        if not os.environ.get("NODECTL_API_TOKEN"):
            self.log.warn("  No NODECTL_API_TOKEN; skip round observation")
            return

        last_eid: Optional[int] = None
        transitions = 0
        max_iters = max(60, self.cfg.observe_rounds * 25)

        for it in range(max_iters):
            data = self._fetch_nodectl_elections_json()
            snap = (data or {}).get("result") or {}
            eid_raw = snap.get("election_id")
            eid = int(eid_raw) if eid_raw is not None else None
            status = (data or {}).get("status")
            elector_n = self._participant_count()

            if isinstance(eid, int) and last_eid is not None and eid != last_eid:
                transitions += 1
                self.log.info(
                    f"  --- Election transition #{transitions}: id {last_eid} → {eid} "
                    f"(elector participants={elector_n}, api_status={status}) ---"
                )

            if isinstance(eid, int):
                last_eid = eid

            self.log.info(
                f"  [observe {it + 1}/{max_iters}] elector_participants={elector_n} "
                f"election_id={eid} status={status}"
            )
            for p in (data or {}).get("our_participants") or []:
                nid = p.get("node_id", "?")
                pool = p.get("pool_addr") or "-"
                st = p.get("status", "?")
                acc = p.get("accepted_stake") or "-"
                sub = len(p.get("stake_submissions") or [])
                self.log.info(
                    f"      {nid:<8} status={st:<12} accepted_stake={acc} "
                    f"pool={pool} submissions={sub}"
                )

            if transitions >= self.cfg.observe_rounds:
                self.log.info(f"  Observed {transitions} election transition(s); done.")
                break
            time.sleep(self.cfg.observe_interval_sec)
        else:
            self.log.warn(
                f"  Observation stopped after {max_iters} polls "
                f"(transitions seen: {transitions}/{self.cfg.observe_rounds})"
            )

        self.log.info("  Pool balances after observation window:")
        self._nctl("config", "pool", "ls")

    # ── Phase 12: Summary ─────────────────────────────────────────────────────

    def phase12_summary(
        self, master_addr: str, wallet_addrs: list, pool_addrs: list, last_count: int
    ) -> None:
        self._phase(12, "Summary")
        rows = [
            ("script",         "run_singlehost_nodectl_snp_toncore.py (2 SNP + 5 TONCore, split50)"),
            ("nodectl pid",    str(self._proc.pid) if self._proc else "N/A"),
            ("nodectl log",    str(self._nodectl_log)),
            ("script log",     str(self.paths.script_log)),
            ("master wallet",  master_addr),
            ("opened wallets", str(len(wallet_addrs))),
            ("opened pools",   str(len(pool_addrs))),
            ("participants",   str(last_count)),
            (
                "observe",
                f"{self.cfg.observe_rounds} transition(s), every {self.cfg.observe_interval_sec}s",
            ),
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
    log.info(f"=== {ts} run_singlehost_nodectl_snp_toncore.py started ===")
    log.info(f"Script log: {paths.script_log}")
    log.info(
        f"Config: NODE_CNT={cfg.node_cnt}, HTTP_API_URL={cfg.http_api_url}, "
        f"CORE_VALIDATOR_DEPOSIT_TON={cfg.core_validator_deposit:g}, "
        f"OBSERVE_ROUNDS={cfg.observe_rounds}, OBSERVE_INTERVAL_SEC={cfg.observe_interval_sec}"
    )

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
    vault_url = f"{paths.vault_file.resolve().as_uri()}?master_key={secrets.token_hex(32)}"
    os.environ["VAULT_URL"] = vault_url
    redacted_vault_url = re.sub(r"(master_key=)[^&]+", r"\1<redacted>", os.environ["VAULT_URL"])
    log.info(f"VAULT_URL={redacted_vault_url}")

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
