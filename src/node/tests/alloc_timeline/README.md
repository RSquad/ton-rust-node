# alloc_timeline

Extracts allocation metrics from the TON node log and displays them as a summary + timeline table.

Useful for diagnosing memory growth: shows which `Alloc` counters are increasing over time.

## Usage

```bash
# From file (recommended for large logs)
python3 alloc_timeline.py node.log

# From stdin
cat node.log | python3 alloc_timeline.py

# Bash version (uses grep + awk)
./alloc_timeline.sh node.log
```

### Filter

By default only metrics containing `Alloc` in the name are shown. Pass a second argument to change the filter:

```bash
python3 alloc_timeline.py node.log "RocksDB"      # only RocksDB metrics
python3 alloc_timeline.py node.log "stored cell"   # specific metric
python3 alloc_timeline.py node.log "socket"         # network throughput

# bash
./alloc_timeline.sh node.log "RocksDB"

# stdin with filter
cat node.log | python3 alloc_timeline.py "RocksDB"
cat node.log | ./alloc_timeline.sh - "RocksDB"
```

### Full timeline (no sampling)

By default the timeline is sampled to ~40 rows. Use `--full` to print every snapshot:

```bash
python3 alloc_timeline.py --full node.log
./alloc_timeline.sh --full node.log
```

## Output

**Summary** — all matching metrics sorted by absolute delta (biggest movers on top):

```
=== SUMMARY (1189 snapshots, 2026-04-07 17:38:57 .. 2026-04-07 19:28:21) ===

METRIC                                            FIRST         LAST        DELTA
------------------------------------------  ------------ ------------ ------------
Alloc NODE stored cells                              909         7825         6916 UP
Alloc OVRL peer stats                                 41          351          310 UP
Alloc RocksDB mem tables, MB                         215          218            3 UP
```

**Timeline** — top 12 movers as columns, with carry-forward for sparse metrics:

```
=== TIMELINE ===

TIME                stored cells peer stats  ...
------------------- ------------ ----------  ...
2026-04-07 17:38:57            -         41  ...
2026-04-07 17:41:51         3504        344  ...
2026-04-07 17:44:45         7041        349  ...
```

## Multiple node restarts

Both scripts detect `Engine::run` in the log and reset state, so only data from the **last node startup** is shown.

## Performance

The python version is ~3x faster than bash on large logs and is recommended for files over 1 GB.
