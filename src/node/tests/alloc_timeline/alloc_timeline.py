#!/usr/bin/env python3
"""Timeline of metrics from TON node log.

Usage: python3 alloc_timeline.py [--full] [logfile] [filter]
  Reads from file if given, otherwise from stdin.
  filter — substring to match in metric names (default: "Alloc").
  --full — print every snapshot (no sampling).
  Only processes data after the last "Engine::run" line.
"""
import os
import re
import sys


def parse_metric_line(line, filter_str):
    """Parse 'MetricName:  cur/ avg/ max[/ total]' -> (name, cur_value) or None."""
    parts = line.split(":")
    if len(parts) < 2:
        return None
    for i in range(len(parts) - 1, 0, -1):
        if re.match(r"^\s*-?\d", parts[i]):
            name = ":".join(parts[:i]).strip()
            values_str = parts[i]
            break
    else:
        return None
    if not name or filter_str not in name:
        return None
    cur_str = values_str.split("/")[0].strip()
    try:
        return name, int(cur_str)
    except ValueError:
        return None


def shorten(name):
    for prefix in ("Alloc NODE ", "Alloc ADNL ", "Alloc OVRL ",
                    "Alloc RLDP ", "Alloc DHT ", "Alloc "):
        if name.startswith(prefix):
            name = name[len(prefix):]
            break
    return name[:18]


def main():
    full = "--full" in sys.argv
    args = [a for a in sys.argv[1:] if not a.startswith("-")]
    path = args[0] if len(args) > 0 and os.path.isfile(args[0]) else None
    filter_str = args[1] if len(args) > 1 else (args[0] if len(args) == 1 and not os.path.isfile(args[0]) else "Alloc")

    if path:
        src = open(path)
    else:
        src = sys.stdin

    ts = None
    midx = {}
    mnames = []
    mfirst = {}
    mlast = {}
    snapshots = []
    cur_ts = None
    cur_vals = {}
    cur_seen = set()

    def flush():
        nonlocal cur_ts, cur_vals, cur_seen
        if cur_ts and cur_vals:
            snapshots.append((cur_ts, cur_vals))
        cur_ts = None
        cur_vals = {}
        cur_seen = set()

    for line in src:
        if "Engine::run" in line:
            flush()
            ts = None
            midx.clear()
            mnames.clear()
            mfirst.clear()
            mlast.clear()
            snapshots.clear()
            continue

        if line.rstrip().endswith("telemetry"):
            parts = line.split()
            if len(parts) >= 2:
                t = parts[0][:10] + " " + parts[1][:8]
                if t[0].isdigit():
                    ts = t
            continue

        if ts is None:
            continue

        parsed = parse_metric_line(line, filter_str)
        if parsed is None:
            continue

        name, val = parsed

        if name not in midx:
            midx[name] = len(mnames)
            mnames.append(name)
        mi = midx[name]

        if mi not in mfirst:
            mfirst[mi] = val
        mlast[mi] = val

        if ts != cur_ts or mi in cur_seen:
            flush()
            cur_ts = ts
        cur_vals[mi] = val
        cur_seen.add(mi)

    flush()

    if src is not sys.stdin:
        src.close()

    if not mnames:
        print("No metrics found.", file=sys.stderr)
        sys.exit(1)

    mcnt = len(mnames)

    deltas = [mlast[i] - mfirst[i] for i in range(mcnt)]
    order = sorted(range(mcnt), key=lambda i: abs(deltas[i]), reverse=True)

    rcnt = len(snapshots)
    ts_first = snapshots[0][0] if snapshots else "?"
    ts_last = snapshots[-1][0] if snapshots else "?"
    print(f"\n=== SUMMARY ({rcnt} snapshots, {ts_first} .. {ts_last}) ===\n")
    print(f"{'METRIC':<42s} {'FIRST':>12s} {'LAST':>12s} {'DELTA':>12s}")
    print(f"{'------------------------------------------':<42s} {'------------':>12s} {'------------':>12s} {'------------':>12s}")
    for i in order:
        d = deltas[i]
        trend = " UP" if d > 0 else (" down" if d < 0 else "")
        print(f"{mnames[i]:<42s} {mfirst[i]:>12d} {mlast[i]:>12d} {d:>12d}{trend}")

    nc = min(mcnt, 12)
    cols = order[:nc]
    short_names = [shorten(mnames[c]) for c in cols]
    widths = [max(len(s), 10) for s in short_names]

    step = 1 if full else max(rcnt // 40, 1)

    print(f"\n=== TIMELINE ===\n")
    header = f"{'TIME':<19s}"
    sep = f"{'-------------------':<19s}"
    for c in range(nc):
        header += f" {short_names[c]:>{widths[c]}s}"
        sep += " " + "-" * widths[c]
    print(header)
    print(sep)

    carry = [None] * nc
    last_printed = -1
    for r in range(rcnt):
        _, vals = snapshots[r]
        for c in range(nc):
            if cols[c] in vals:
                carry[c] = vals[cols[c]]
        if r % step != 0 and r != rcnt - 1:
            continue
        row = f"{snapshots[r][0]:<19s}"
        for c in range(nc):
            if carry[c] is not None:
                row += f" {carry[c]:>{widths[c]}d}"
            else:
                row += f" {'-':>{widths[c]}s}"
        print(row)
        last_printed = r

    if last_printed != rcnt - 1:
        row = f"{snapshots[-1][0]:<19s}"
        for c in range(nc):
            if carry[c] is not None:
                row += f" {carry[c]:>{widths[c]}d}"
            else:
                row += f" {'-':>{widths[c]}s}"
        print(row)


if __name__ == "__main__":
    main()
