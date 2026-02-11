# Grafana Dashboards

Dashboards are defined as TypeScript code using [Grafana Foundation SDK](https://grafana.com/docs/grafana/latest/as-code/observability-as-code/foundation-sdk/) and compiled to JSON.

## Dashboards

| Source | Description |
|--------|-------------|
| `src/index.ts` | TON Node Overview — sync status, TPS, validation, collation, network, database |

The dashboard uses two template variables at the top: **network** and **node_id**, both support multi-select. These correspond to `global_labels` in the node's metrics config.

For the full list of metrics and PromQL examples see [metrics.md](../helm/ton-rust-node/docs/metrics.md).

### Sections

- **Node Status** — sync status, validation status, MC timediff, next round prediction
- **Build Info** — version, commit, rust version, branch
- **TPS** — transactions per second at 10s / 5m / 30m windows
- **Sync & Block Progress** — MC timediff over time, block seqno, external messages queue
- **Validation & Collation** — active validators/collators, success/fail rates, duration percentiles, gas, message throughput
- **Outbound Message Queue** — enqueue/dequeue rates, cleanup stats
- **Network** — ADNL roundtrip, catchain queries, neighbour failures, overlay query time
- **Database & Storage** — shard state queue, operation duration percentiles

## Generate

We recommend [Bun](https://bun.sh/), but any Node.js-compatible runtime works (`npm`, `yarn`, `pnpm`, etc.):

```bash
cd grafana
bun install
bun run generate          # outputs ton-node-overview.json
```

## Import into Grafana

1. **Dashboards** > **New** > **Import**
2. Upload `ton-node-overview.json`
3. Select your Prometheus datasource
4. **Import**

## Editing

1. Edit the relevant `src/*.ts` file
2. Run `bun run generate` to regenerate the JSON
3. Import into Grafana to verify
4. Commit the TypeScript source (JSON is gitignored)
