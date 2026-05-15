#!/bin/bash
# alloc_timeline.sh — Timeline of metrics from TON node log.
# Usage: ./alloc_timeline.sh [--full] [logfile] [filter]
#   Reads from file if given, otherwise from stdin.
#   filter — substring to match in metric names (default: "Alloc").
#   --full — print every snapshot (no sampling).
#   Only shows data after the last "Engine::run" line.

set -euo pipefail

FULL=0
POSITIONAL=()
for arg in "$@"; do
    case "$arg" in
        --full) FULL=1 ;;
        *) POSITIONAL+=("$arg") ;;
    esac
done
INPUT="${POSITIONAL[0]:--}"
FILTER="${POSITIONAL[1]:-Alloc}"
if [ "$INPUT" != "-" ] && [ ! -f "$INPUT" ]; then
    echo "File not found: $INPUT" >&2
    exit 1
fi

grep -E "Engine::run|telemetry$|${FILTER}" "$INPUT" \
| awk -v filter="$FILTER" -v full="$FULL" '
BEGIN { mcnt = 0; rcnt = 0; cur_ts = "" }
/Engine::run/ {
    # Reset first stage
    ts = ""
    # Reset second stage
    delete midx; delete mname; delete mfirst; delete mlast
    delete cur_vals; delete cur_seen; delete rts; delete rdata
    mcnt = 0; rcnt = 0; cur_ts = ""
    next
}
/telemetry$/ {
    t = substr($1, 1, 10) " " substr($2, 1, 8)
    if (t ~ /^[0-9]/) ts = t
    next
}
{
    if (ts == "") next
    line = $0
    n = split(line, parts, ":")
    if (n < 2) next
    name = ""; values = ""
    for (i = n; i >= 1; i--) {
        if (match(parts[i], /^ *-?[0-9]/)) {
            values = parts[i]
            for (j = 1; j < i; j++) {
                if (j > 1) name = name ":"
                name = name parts[j]
            }
            break
        }
    }
    if (values == "" || name == "") next
    gsub(/^ +| +$/, "", name)
    if (name == "") next
    if (index(name, filter) == 0) next
    split(values, v, "/")
    gsub(/^ +| +$/, "", v[1])
    val = v[1] + 0

    if (!(name in midx)) {
        midx[name] = mcnt
        mname[mcnt] = name
        mcnt++
    }

    mi = midx[name]
    if (!(mi in mfirst)) mfirst[mi] = val
    mlast[mi] = val

    if (ts != cur_ts || (mi in cur_seen)) {
        if (cur_ts != "") {
            rts[rcnt] = cur_ts
            for (k in cur_vals) rdata[rcnt, k] = cur_vals[k]
            rcnt++
            delete cur_vals
            delete cur_seen
        }
        cur_ts = ts
    }
    cur_vals[mi] = val
    cur_seen[mi] = 1
}
END {
    if (cur_ts != "") {
        rts[rcnt] = cur_ts
        for (k in cur_vals) rdata[rcnt, k] = cur_vals[k]
        rcnt++
    }
    if (mcnt == 0) { print "No metrics found." > "/dev/stderr"; exit 1 }

    for (i = 0; i < mcnt; i++) {
        mdelta[i] = mlast[i] - mfirst[i]
        mabs[i] = mdelta[i] < 0 ? -mdelta[i] : mdelta[i]
        ord[i] = i
    }
    for (i = 1; i < mcnt; i++) {
        tmp = ord[i]; j = i - 1
        while (j >= 0 && mabs[ord[j]] < mabs[tmp]) { ord[j+1] = ord[j]; j-- }
        ord[j+1] = tmp
    }

    printf "\n=== SUMMARY (%d snapshots, %s .. %s) ===\n\n", rcnt, rts[0], rts[rcnt-1]
    printf "%-42s %12s %12s %12s\n", "METRIC", "FIRST", "LAST", "DELTA"
    printf "%-42s %12s %12s %12s\n", "------------------------------------------", "------------", "------------", "------------"
    for (i = 0; i < mcnt; i++) {
        mi = ord[i]; d = mdelta[mi]
        trend = d > 0 ? " UP" : (d < 0 ? " down" : "")
        printf "%-42s %12d %12d %12d%s\n", mname[mi], mfirst[mi], mlast[mi], d, trend
    }

    nc = mcnt < 12 ? mcnt : 12
    for (c = 0; c < nc; c++) {
        ci = ord[c]
        sh = mname[ci]
        sub(/^Alloc NODE /, "", sh); sub(/^Alloc ADNL /, "", sh)
        sub(/^Alloc OVRL /, "", sh); sub(/^Alloc RLDP /, "", sh)
        sub(/^Alloc DHT /,  "", sh); sub(/^Alloc /,      "", sh)
        if (length(sh) > 18) sh = substr(sh, 1, 18)
        sn[c] = sh; ci_map[c] = ci
        w = length(sh); if (w < 10) w = 10; sw[c] = w
    }

    if (full) step = 1; else { step = int(rcnt / 40); if (step < 1) step = 1 }

    printf "\n=== TIMELINE ===\n\n"
    printf "%-19s", "TIME"
    for (c = 0; c < nc; c++) printf " %" sw[c] "s", sn[c]
    printf "\n%-19s", "-------------------"
    for (c = 0; c < nc; c++) { s=""; for(k=0;k<sw[c];k++) s=s"-"; printf " %s", s }
    printf "\n"

    for (c = 0; c < nc; c++) cf[c] = ""
    lp = -1
    for (r = 0; r < rcnt; r++) {
        for (c = 0; c < nc; c++) {
            key = r SUBSEP ci_map[c]
            if (key in rdata) cf[c] = rdata[key]
        }
        if (r % step != 0 && r != rcnt - 1) continue
        printf "%-19s", rts[r]
        for (c = 0; c < nc; c++) {
            if (cf[c] != "") printf " %" sw[c] "d", cf[c]
            else printf " %" sw[c] "s", "-"
        }
        printf "\n"; lp = r
    }
    if (lp != rcnt - 1) {
        printf "%-19s", rts[rcnt-1]
        for (c = 0; c < nc; c++) {
            if (cf[c] != "") printf " %" sw[c] "d", cf[c]
            else printf " %" sw[c] "s", "-"
        }
        printf "\n"
    }
}
'
