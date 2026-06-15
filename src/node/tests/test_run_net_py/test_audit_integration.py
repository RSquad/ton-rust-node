#!/usr/bin/env python3
"""
Audit-log integration test suite for nodectl.

Can be run stand-alone after run_singlehost_nodectl.py has brought up the
service, or invoked as a phase from inside that bootstrap script.

Exit code: 0 — all required checks pass; 1 — one or more failures.

Required env vars (or CLI flags):
  CONFIG_PATH        — path to the running nodectl-config.json
  NODECTL_API_TOKEN  — JWT token for REST calls (produced by phase 8 auth setup)

Optional env vars:
  AUDIT_LOG_PATH     — override audit file path (default: read from config,
                       fall back to <config_dir>/logs/audit.jsonl)
  NODECTL_REST_URL   — override REST base URL (default: read from config http.bind)

Usage:
  python3 test_audit_integration.py [--config /path/to/nodectl-config.json]
                                    [--rest-url http://127.0.0.1:8080]
                                    [--audit-log /path/to/audit.jsonl]
                                    [--verbose]
"""

from __future__ import annotations

import argparse
import datetime
import json
import os
import re
import sys
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any, Optional

# ── RFC 3339 millis with Z: 2026-05-22T12:10:30.123Z ─────────────────────────
_TS_PATTERN = re.compile(
    r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$"
)

REQUIRED_EVENT_FIELDS = ("id", "ts", "outcome", "event_type", "actor", "target")
VALID_OUTCOMES = {"success", "failure", "skipped"}

# Canonical set of event_type values produced by the current service version.
# Events outside this set indicate schema drift (removed/renamed variant).
KNOWN_EVENT_TYPES = {
    "elections.key_generated",
    "elections.stake_submitted",
    "elections.stake_accepted",
    "elections.stake_skipped",
    "elections.stake_failed",
    "elections.stake_recovered",
    "elections.stake_recover_failed",
    "elections.withdraw_processed",
    "elections.withdraw_failed",
    "rewards.distribution_started",
    "rewards.distribution_completed",
    "rewards.distribution_failed",
    "rewards.recipient_skipped",
    "rest_api.config_updated",
    "rest_api.auth_login_succeeded",
    "rest_api.auth_login_rejected",
    "rest_api.token_rejected",
    "vault.key_created",
    "vault.key_removed",
    "system.service_started",
    "system.service_stopped",
    "system.audit_events_dropped",
}


# ══════════════════════════════════════════════════════════════════════════════
# Tiny colour logger
# ══════════════════════════════════════════════════════════════════════════════

class Logger:
    def __init__(self, verbose: bool = False) -> None:
        self.verbose = verbose

    def _emit(self, colour: str, label: str, msg: str) -> None:
        print(f"\033[{colour}m[{label}]\033[0m {msg}", flush=True)

    def pass_(self, msg: str) -> None: self._emit("32", "PASS", msg)
    def fail(self, msg: str) -> None:  self._emit("31", "FAIL", msg)
    def skip(self, msg: str) -> None:  self._emit("33", "SKIP", msg)
    def info(self, msg: str) -> None:  self._emit("36", "INFO", msg)
    def debug(self, msg: str) -> None:
        if self.verbose:
            self._emit("37", "DBG ", msg)


# ══════════════════════════════════════════════════════════════════════════════
# Result accumulator
# ══════════════════════════════════════════════════════════════════════════════

class Results:
    def __init__(self) -> None:
        self.passed = 0
        self.failed = 0
        self.skipped = 0

    def record_pass(self, log: Logger, msg: str) -> None:
        self.passed += 1
        log.pass_(msg)

    def record_fail(self, log: Logger, msg: str) -> None:
        self.failed += 1
        log.fail(msg)

    def record_skip(self, log: Logger, msg: str) -> None:
        self.skipped += 1
        log.skip(msg)

    @property
    def ok(self) -> bool:
        return self.failed == 0

    def summary(self, log: Logger) -> None:
        colour = "32" if self.ok else "31"
        log._emit(colour, "SUM",
                  f"passed={self.passed}  failed={self.failed}  skipped={self.skipped}")


# ══════════════════════════════════════════════════════════════════════════════
# Config / path resolution
# ══════════════════════════════════════════════════════════════════════════════

def resolve_audit_log_path(config_path: Path) -> Path:
    """
    Find the audit.jsonl path using the following priority:

    1. $AUDIT_LOG_PATH env var
    2. `audit.path` field in the config file
       - absolute path used as-is
       - relative path resolved against: config-dir, then CWD, first that exists
    3. Default candidates (first that exists wins):
       - ./logs/audit.jsonl  (service CWD — matches the nodectl default)
       - <config_dir>/logs/audit.jsonl
    """
    env_override = os.environ.get("AUDIT_LOG_PATH", "").strip()
    if env_override:
        return Path(env_override)

    try:
        cfg = json.loads(config_path.read_text())
        audit_path_str = cfg.get("audit", {}).get("path", "")
        if audit_path_str:
            p = Path(audit_path_str)
            if p.is_absolute():
                return p
            # Relative: try config-dir first, then CWD
            candidate_cfg = config_path.parent / p
            if candidate_cfg.exists():
                return candidate_cfg
            candidate_cwd = Path.cwd() / p
            if candidate_cwd.exists():
                return candidate_cwd
            return candidate_cfg  # return best-guess even if missing
    except Exception:
        pass

    # Default: service writes to ./logs/audit.jsonl relative to its CWD.
    # The bootstrap script starts nodectl from test_run_net_py/, so that
    # matches <script_dir>/logs/audit.jsonl. Fall back to config-dir if
    # the CWD candidate does not exist.
    cwd_candidate = Path.cwd() / "logs" / "audit.jsonl"
    if cwd_candidate.exists():
        return cwd_candidate

    cfg_candidate = config_path.parent / "logs" / "audit.jsonl"
    if cfg_candidate.exists():
        return cfg_candidate

    # Neither exists — return CWD candidate so the error message is meaningful
    return cwd_candidate


def resolve_rest_base_url(config_path: Path) -> str:
    """Derive REST base URL from config http.bind or env."""
    env_override = os.environ.get("NODECTL_REST_URL", "").strip()
    if env_override:
        return env_override.rstrip("/")

    try:
        cfg = json.loads(config_path.read_text())
        bind = str(cfg.get("http", {}).get("bind", "127.0.0.1:8080"))
    except Exception:
        bind = "127.0.0.1:8080"

    if bind.startswith("["):
        bracket_end = bind.find("]")
        host = bind[1:bracket_end] if bracket_end > 0 else "127.0.0.1"
        rest = bind[bracket_end + 1:].lstrip() if bracket_end > 0 else ""
        port = rest[1:] if rest.startswith(":") else "8080"
    elif bind.count(":") == 1:
        host, port = bind.split(":", 1)
    else:
        host, port = "127.0.0.1", "8080"

    if host in ("0.0.0.0", "::"):
        host = "127.0.0.1"

    return (f"http://[{host}]:{port}" if ":" in host else f"http://{host}:{port}").rstrip("/")


def rest_get_json(base_url: str, path: str, token: str, timeout: int = 15) -> dict:
    url = base_url + path
    req = urllib.request.Request(url, headers={"Authorization": f"Bearer {token}"})
    body = ""
    with urllib.request.urlopen(req, timeout=timeout) as resp:
        body = resp.read().decode(errors="replace")
    return json.loads(body)


# ══════════════════════════════════════════════════════════════════════════════
# JSONL parsing helpers
# ══════════════════════════════════════════════════════════════════════════════

def read_jsonl(path: Path) -> tuple[list[dict], list[str]]:
    """
    Returns (records, errors). Each record is a parsed JSON object.
    errors contains descriptions of lines that failed to parse.
    """
    records: list[dict] = []
    errors: list[str] = []
    for i, raw in enumerate(path.read_text(encoding="utf-8").splitlines(), start=1):
        raw = raw.strip()
        if not raw:
            continue
        try:
            obj = json.loads(raw)
            if not isinstance(obj, dict):
                errors.append(f"line {i}: not a JSON object: {raw[:120]}")
            else:
                records.append(obj)
        except json.JSONDecodeError as e:
            errors.append(f"line {i}: {e.msg}: {raw[:120]}")
    return records, errors




# ══════════════════════════════════════════════════════════════════════════════
# Test suites
# ══════════════════════════════════════════════════════════════════════════════

class AuditFileTests:
    """Tests that inspect the audit.jsonl file on disk."""

    def __init__(self, audit_path: Path, log: Logger, results: Results) -> None:
        self.path = audit_path
        self.log = log
        self.r = results
        self._records: list[dict] = []
        self._events: list[dict] = []

    def run_all(self) -> None:
        self.log.info(f"=== Audit file checks: {self.path} ===")

        if not self._check_file_exists():
            return  # nothing to test without the file

        records, errors = read_jsonl(self.path)

        if errors:
            for e in errors:
                self.r.record_fail(self.log, f"JSONL parse error — {e}")
        else:
            self.r.record_pass(self.log, "All lines parse as valid JSON objects")

        self._records = records
        # Unified JSONL format: every line is an event carrying `event_type`.
        # Guard against any stray line without it (e.g. a legacy header artifact).
        self._events = [r for r in records if "event_type" in r]

        self._check_file_header(records)
        self._check_event_fields()
        self._check_ts_format()
        self._check_outcome_values()
        self._check_service_started_present()
        self._check_no_dedup_duplicates()
        self._check_actor_shape()
        self._check_no_unknown_event_types()
        self._check_target_shape()

    # ── individual checks ─────────────────────────────────────────────────────

    def _check_file_exists(self) -> bool:
        if self.path.exists():
            size = self.path.stat().st_size
            self.r.record_pass(self.log, f"audit.jsonl exists ({size} bytes)")
            return True
        self.r.record_fail(self.log, f"audit.jsonl not found: {self.path}")
        return False

    def _check_file_header(self, records: list[dict]) -> None:
        """First line must be a system.service_started event with version and host fields."""
        if not records:
            self.r.record_fail(self.log, "audit.jsonl is empty — no events at all")
            return

        first = records[0]
        et = first.get("event_type")
        if et != "system.service_started":
            self.r.record_fail(self.log,
                f"First line must be system.service_started, got event_type={et!r}")
            return

        data = first.get("data", {})
        missing = [f for f in ("version", "host") if f not in data]
        if missing:
            self.r.record_fail(self.log,
                f"system.service_started missing data fields: {missing}")
        else:
            self.r.record_pass(self.log,
                f"First event is system.service_started "
                f"(version={data.get('version')!r}, host={data.get('host')!r})")

    def _check_event_fields(self) -> None:
        if not self._events:
            self.r.record_skip(self.log, "No events in file — skipping field checks")
            return
        bad: list[str] = []
        for ev in self._events:
            missing = [f for f in REQUIRED_EVENT_FIELDS if f not in ev]
            if missing:
                bad.append(f"event_type={ev.get('event_type')!r} id={ev.get('id')!r}: "
                           f"missing {missing}")
        if bad:
            for b in bad[:5]:
                self.r.record_fail(self.log, f"Event missing required fields — {b}")
            if len(bad) > 5:
                self.r.record_fail(self.log, f"  … and {len(bad) - 5} more")
        else:
            self.r.record_pass(self.log,
                f"All {len(self._events)} events have required fields "
                f"({', '.join(REQUIRED_EVENT_FIELDS)})")

    def _check_ts_format(self) -> None:
        if not self._events:
            return
        bad: list[str] = []
        for ev in self._events:
            ts = ev.get("ts", "")
            if not _TS_PATTERN.match(ts):
                bad.append(f"event_type={ev.get('event_type')!r}: ts={ts!r}")
        if bad:
            for b in bad[:5]:
                self.r.record_fail(self.log, f"Bad ts format — {b}")
        else:
            self.r.record_pass(self.log,
                f"All {len(self._events)} events have RFC3339-millis-Z timestamps")

    def _check_outcome_values(self) -> None:
        if not self._events:
            return
        bad: list[str] = []
        for ev in self._events:
            o = ev.get("outcome")
            if o not in VALID_OUTCOMES:
                bad.append(f"event_type={ev.get('event_type')!r}: outcome={o!r}")
        if bad:
            for b in bad[:5]:
                self.r.record_fail(self.log, f"Invalid outcome — {b}")
        else:
            self.r.record_pass(self.log,
                f"All {len(self._events)} events have valid outcome values")

    def _check_service_started_present(self) -> None:
        found = any(ev.get("event_type") == "system.service_started"
                    for ev in self._events)
        if found:
            self.r.record_pass(self.log, "system.service_started event present")
        else:
            self.r.record_fail(self.log,
                "system.service_started event not found in audit.jsonl "
                "(expected on service startup)")

    def _check_no_unknown_event_types(self) -> None:
        unknown = {
            ev.get("event_type") for ev in self._events
            if ev.get("event_type") not in KNOWN_EVENT_TYPES
        }
        if unknown:
            for et in sorted(unknown):
                self.r.record_fail(self.log,
                    f"Unknown event_type {et!r} — removed from schema or schema drift "
                    f"(check AuditEventPayload enum)")
        else:
            self.r.record_pass(self.log,
                f"All {len(self._events)} events use known event_type values")

    def _check_no_dedup_duplicates(self) -> None:
        """No two stake_skipped events with the same (node_id, election_id, reason)."""
        skipped_events = [
            ev for ev in self._events
            if ev.get("event_type") == "elections.stake_skipped"
        ]
        if not skipped_events:
            self.r.record_skip(self.log,
                "No elections.stake_skipped events — dedup check skipped")
            return

        seen: set[tuple] = set()
        dups: list[str] = []
        for ev in skipped_events:
            target = ev.get("target", {})
            node_id = target.get("id", "")
            election_id = target.get("election_id")
            data = ev.get("data", {})
            reason = data.get("reason", "")
            key = (node_id, election_id, reason)
            if key in seen:
                dups.append(
                    f"node_id={node_id!r} election_id={election_id} reason={reason!r}"
                )
            else:
                seen.add(key)

        if dups:
            for d in dups[:5]:
                self.r.record_fail(self.log,
                    f"Duplicate stake_skipped in file (dedup failed) — {d}")
        else:
            self.r.record_pass(self.log,
                f"No duplicate stake_skipped events among {len(skipped_events)} "
                f"(unique keys: {len(seen)})")

    def _check_actor_shape(self) -> None:
        if not self._events:
            return
        bad: list[str] = []
        for ev in self._events:
            actor = ev.get("actor")
            if not isinstance(actor, dict):
                bad.append(f"event_type={ev.get('event_type')!r}: actor is not an object")
                continue
            if "kind" not in actor:
                bad.append(f"event_type={ev.get('event_type')!r}: actor missing 'kind'")
        if bad:
            for b in bad[:5]:
                self.r.record_fail(self.log, f"Bad actor shape — {b}")
        else:
            self.r.record_pass(self.log, f"All {len(self._events)} actors have 'kind' field")

    def _check_target_shape(self) -> None:
        if not self._events:
            return
        bad: list[str] = []
        for ev in self._events:
            target = ev.get("target")
            if not isinstance(target, dict):
                bad.append(f"event_type={ev.get('event_type')!r}: target is not an object")
                continue
            if "kind" not in target:
                bad.append(f"event_type={ev.get('event_type')!r}: target missing 'kind'")
        if bad:
            for b in bad[:5]:
                self.r.record_fail(self.log, f"Bad target shape — {b}")
        else:
            self.r.record_pass(self.log, f"All {len(self._events)} targets have 'kind' field")

    # ── summary helpers ────────────────────────────────────────────────────────

    def print_event_type_counts(self) -> None:
        if not self._events:
            return
        counts: dict[str, int] = {}
        for ev in self._events:
            et = ev.get("event_type", "<unknown>")
            counts[et] = counts.get(et, 0) + 1
        self.log.info("Event type distribution:")
        for et, n in sorted(counts.items()):
            self.log.info(f"  {n:4d}  {et}")


class AuditRestTests:
    """Tests that call GET /v1/elections and validate audit-enriched fields."""

    def __init__(
        self,
        rest_url: str,
        token: str,
        log: Logger,
        results: Results,
    ) -> None:
        self.rest_url = rest_url
        self.token = token
        self.log = log
        self.r = results

    def run_all(self) -> None:
        self.log.info(f"=== REST API checks: {self.rest_url}/v1/elections ===")

        data = self._fetch_elections()
        if data is None:
            return

        self._check_response_shape(data)
        self._check_recent_events_empty(data)
        self._check_participants_structure(data)
        self._check_stake_submissions_no_duplicates(data)

    # ── individual checks ─────────────────────────────────────────────────────

    def _fetch_elections(self) -> Optional[dict]:
        try:
            data = rest_get_json(self.rest_url, "/v1/elections", self.token)
            self.r.record_pass(self.log, "GET /v1/elections → HTTP 200")
            return data
        except urllib.error.HTTPError as e:
            body = e.read().decode(errors="replace")[:400]
            self.r.record_fail(self.log,
                f"GET /v1/elections → HTTP {e.code}: {body}")
        except urllib.error.URLError as e:
            self.r.record_fail(self.log,
                f"GET /v1/elections connection failed: {e.reason}")
        except json.JSONDecodeError as e:
            self.r.record_fail(self.log,
                f"GET /v1/elections returned invalid JSON: {e.msg}")
        return None

    def _check_response_shape(self, data: dict) -> None:
        # Accept both {ok, result} and flat shapes (depending on version)
        result = data.get("result", data)
        if not isinstance(result, dict):
            self.r.record_fail(self.log,
                f"elections response result is not an object: {type(result)}")
            return

        has_election_id = "election_id" in result or "election_id" in data
        has_participants = (
            "our_participants" in result or "our_participants" in data
        )
        if has_participants:
            self.r.record_pass(self.log, "elections response contains 'our_participants'")
        else:
            self.r.record_fail(self.log,
                "elections response has no 'our_participants' field")

    def _check_recent_events_empty(self, data: dict) -> None:
        """recent_events must be absent or an empty list (sma-106 skips serialization)."""
        result = data.get("result", data)
        recent = result.get("recent_events")
        if recent is None:
            self.r.record_pass(self.log,
                "recent_events absent from response (skip_serializing_if = empty)")
        elif isinstance(recent, list) and len(recent) == 0:
            self.r.record_pass(self.log, "recent_events is present but empty []")
        else:
            self.r.record_fail(self.log,
                f"recent_events is unexpected: {str(recent)[:200]}")

    def _check_participants_structure(self, data: dict) -> None:
        result = data.get("result", data)
        participants = result.get("our_participants", [])
        if not isinstance(participants, list):
            self.r.record_fail(self.log,
                f"our_participants is not a list: {type(participants)}")
            return
        if not participants:
            self.r.record_skip(self.log,
                "our_participants is empty — skipping per-participant checks")
            return

        bad_last_error: list[str] = []
        for p in participants:
            node_id = p.get("node_id", "?")
            last_error = p.get("last_error")
            if last_error is not None and not isinstance(last_error, str):
                bad_last_error.append(
                    f"node_id={node_id!r}: last_error is not a string: {last_error!r}"
                )

        if bad_last_error:
            for b in bad_last_error:
                self.r.record_fail(self.log, f"Bad last_error type — {b}")
        else:
            with_errors = [p for p in participants if p.get("last_error")]
            self.r.record_pass(self.log,
                f"our_participants: {len(participants)} participant(s), "
                f"{len(with_errors)} with last_error populated")
            if with_errors:
                for p in with_errors:
                    self.log.info(
                        f"  node_id={p.get('node_id')!r}  "
                        f"last_error={p.get('last_error')!r}"
                    )

    def _check_stake_submissions_no_duplicates(self, data: dict) -> None:
        result = data.get("result", data)
        participants = result.get("our_participants", [])
        if not isinstance(participants, list) or not participants:
            return

        dups: list[str] = []
        for p in participants:
            node_id = p.get("node_id", "?")
            subs = p.get("stake_submissions") or []
            seen: set[tuple] = set()
            for s in subs:
                key = (s.get("stake"), s.get("submission_time"))
                if key in seen:
                    dups.append(
                        f"node_id={node_id!r} stake={key[0]!r} time={key[1]!r}"
                    )
                else:
                    seen.add(key)

        if dups:
            for d in dups[:5]:
                self.r.record_fail(self.log,
                    f"Duplicate stake_submission in REST response — {d}")
        else:
            total_subs = sum(len(p.get("stake_submissions") or [])
                             for p in participants)
            self.r.record_pass(self.log,
                f"No duplicate stake_submissions across {len(participants)} "
                f"participant(s) ({total_subs} total)")


# ══════════════════════════════════════════════════════════════════════════════
# Entry point
# ══════════════════════════════════════════════════════════════════════════════

def parse_args() -> argparse.Namespace:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--config", metavar="PATH",
                    default=os.environ.get("CONFIG_PATH", ""),
                    help="Path to nodectl-config.json "
                         "(default: $CONFIG_PATH)")
    ap.add_argument("--rest-url", metavar="URL",
                    default="",
                    help="Base REST URL, e.g. http://127.0.0.1:8080 "
                         "(default: derive from config http.bind or $NODECTL_REST_URL)")
    ap.add_argument("--audit-log", metavar="PATH",
                    default="",
                    help="Override audit.jsonl path "
                         "(default: derive from config or $AUDIT_LOG_PATH)")
    ap.add_argument("--verbose", "-v", action="store_true",
                    help="Print debug lines")
    return ap.parse_args()


def main() -> None:
    args = parse_args()
    log = Logger(verbose=args.verbose)
    results = Results()

    # ── Resolve config path ────────────────────────────────────────────────────
    config_path_str = args.config or os.environ.get("CONFIG_PATH", "")
    if not config_path_str:
        log._emit("31", "FATAL",
                  "CONFIG_PATH is not set. "
                  "Pass --config or set the CONFIG_PATH environment variable.")
        sys.exit(1)

    config_path = Path(config_path_str)
    if not config_path.exists():
        log._emit("31", "FATAL", f"Config file not found: {config_path}")
        sys.exit(1)

    # ── Resolve audit log path ─────────────────────────────────────────────────
    if args.audit_log:
        audit_path = Path(args.audit_log)
    elif os.environ.get("AUDIT_LOG_PATH"):
        audit_path = Path(os.environ["AUDIT_LOG_PATH"])
    else:
        audit_path = resolve_audit_log_path(config_path)

    # ── Resolve REST URL ───────────────────────────────────────────────────────
    rest_url = (args.rest_url or "").rstrip("/") or resolve_rest_base_url(config_path)

    ts = datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%d %H:%M:%S UTC")
    log.info(f"=== {ts} test_audit_integration.py ===")
    log.info(f"config:    {config_path}")
    log.info(f"audit log: {audit_path}")
    log.info(f"rest url:  {rest_url}")

    # ── Suite 1: audit.jsonl file ─────────────────────────────────────────────
    file_tests = AuditFileTests(audit_path, log, results)
    file_tests.run_all()
    file_tests.print_event_type_counts()

    # ── Suite 2: REST API ─────────────────────────────────────────────────────
    token = os.environ.get("NODECTL_API_TOKEN", "").strip()
    if not token:
        results.record_skip(log,
            "NODECTL_API_TOKEN not set — skipping all REST API checks")
    else:
        rest_tests = AuditRestTests(rest_url, token, log, results)
        rest_tests.run_all()

    # ── Final summary ─────────────────────────────────────────────────────────
    print()
    results.summary(log)
    sys.exit(0 if results.ok else 1)


if __name__ == "__main__":
    main()
