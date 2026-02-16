# Maintaining the chart

Notes for chart maintainers.

## Table of contents

- [Publishing the chart](#publishing-the-chart)
- [Documentation guidelines](#documentation-guidelines)
- [Updating the Parameters table in README](#updating-the-parameters-table-in-readme)
- [Formatting conventions](#formatting-conventions)

---

## Publishing the chart

The chart is published as an OCI artifact to GitHub Container Registry (GHCR).

### Prerequisites

Login to GHCR with a token that has `write:packages` scope:

```bash
# Using gh CLI
echo $(gh auth token) | helm registry login ghcr.io -u $(gh auth status --json login -q .login) --password-stdin

# Using a personal access token
echo $GITHUB_TOKEN | helm registry login ghcr.io -u <your-github-username> --password-stdin
```

### Publishing a new version

1. Update `version` in `Chart.yaml`.
2. Package and push:

```bash
# From the repository root
helm package helm/ton-rust-node
helm push ton-rust-node-<version>.tgz oci://ghcr.io/rsquad/helm
```

3. Verify the published chart:

```bash
helm show chart oci://ghcr.io/rsquad/helm/ton-rust-node --version <version>
```

### Installing from the registry

```bash
helm install my-node oci://ghcr.io/rsquad/helm/ton-rust-node --version <version> -f values.yaml
```

---

## Documentation guidelines

All documentation lives in the `docs/` directory:

| File | Content |
|------|---------|
| `node-config.md` | `config.json` field reference, JSON examples, archival setup |
| `logging.md` | log4rs config (appenders, levels, rotation) |
| `global-config.md` | Global network config overview |
| `networking.md` | Networking modes (LoadBalancer, NodePort, hostPort, hostNetwork), NetworkPolicy |
| `resources.md` | CPU and memory recommendations |
| `metrics.md` | Prometheus metrics reference (all 51 metrics, labels) |
| `monitoring.md` | Prometheus/Grafana setup (ServiceMonitor, annotations, alerts) |
| `probes.md` | Kubernetes health probes (/healthz, /readyz) |

### General principles

- **Audience** — operators who deploy and configure TON nodes on Kubernetes. Assume familiarity with Helm and containers, but not with TON internals.
- **Be precise** — document what the software actually does, not what it might do. If you are unsure about a field's behavior, verify it before writing.
- **Defaults are code defaults** — the "Default" column in field reference tables must show what the node uses when the field is omitted from the config. Do not put recommended values there.
- **Examples are recommendations** — JSON examples show sensible production configurations. They may intentionally differ from code defaults. A note above the examples makes this explicit.
- **Keep it flat** — prefer short paragraphs and tables over deeply nested prose. One idea per paragraph.
- **Do not duplicate** — if something is documented in one file, link to it from others rather than repeating it.

### Editing workflow

1. Find the right file (see table above).
2. Make the change, following the [formatting conventions](#formatting-conventions) below.
3. If the change affects JSON examples **and** the field reference, update both. They serve different purposes (recommendations vs. code defaults) but must not contradict each other without an explicit note.
4. If you changed `values.yaml`, regenerate the README parameters table (see [next section](#updating-the-parameters-table-in-readme)).

---

## Updating the Parameters table in README

The Parameters section in README.md is auto-generated from `@param` annotations in `values.yaml` using [bitnami/readme-generator-for-helm](https://github.com/bitnami/readme-generator-for-helm).

**Do not edit the Parameters section by hand** — your changes will be overwritten on the next generation run.

### Install

```bash
npm install -g @bitnami/readme-generator-for-helm
```

### Run

```bash
readme-generator -v helm/ton-rust-node/values.yaml -r helm/ton-rust-node/README.md
```

The tool finds the `## Parameters` heading in README.md and replaces everything up to the next `##`-level heading with tables generated from the annotations.

### Annotation format

In `values.yaml`, every documented parameter needs a `## @param` comment:

```yaml
## @param storage.db.size Database volume size (hundreds of GB for mainnet)
```

Group parameters into sections with `## @section`:

```yaml
## @section Storage parameters
```

Use modifiers in square brackets when the default display needs help:

| Modifier | When to use | Example |
|----------|------------|---------|
| `[object]` | Default is `{}` | `## @param probes [object] Liveness and readiness probes` |
| `[array]` | Default is `[]` | `## @param command [array] Container command` |
| `[nullable]` | Default is `null` | `## @param ports.liteserver [nullable] Liteserver port` |
| `[default: text]` | Show custom text instead of the actual value | `## @param globalConfig [default: bundled mainnet] Global config` |

To exclude a parameter from the table, use `## @skip`:

```yaml
## @skip podDisruptionBudget.maxUnavailable
```

### Workflow

1. Edit `values.yaml` — add/change parameters and their `@param` annotations
2. Run `readme-generator` (command above)
3. Verify the generated table looks correct
4. Commit both files together

---

## Formatting conventions

Reference for the style used across all docs.

### Document structure

Each doc follows a consistent layout:

1. **H1 title** — exactly one per file
2. **Table of contents** — anchor links to all H2 sections
3. **H2 sections** — major topics
4. **H3/H4 subsections** — nested details, individual fields

### Heading levels

| Level | Use for |
|-------|---------|
| `#` | Document title (one per file) |
| `##` | Major sections (Table of contents, Field reference, Examples) |
| `###` | Subsections, config object names (e.g. `gc`, `collator_config`) |
| `####` | Individual fields (e.g. `gc.enable_for_archives`) |

Use `---` (horizontal rule) between H3 sections for visual separation.

### Field reference format

Each field gets an H4 heading with the full dotted path in backticks.

**Object fields** — use a 4-column table for child fields:

```markdown
#### `gc.cells_gc_config`

Prose description of the field — what it does and when you need it.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `gc_interval_sec` | u32 | `900` (15 min) | How often cell GC runs |
```

**Simple fields** — use a type/required/default table:

```markdown
#### `sync_by_archives`

Description text.

| Type | Required | Default |
|------|----------|---------|
| bool | no | `true` |
```

Rules:

- Description comes **before** the table.
- Additional notes or recommendations go **after** the table.

### Types

Write types lowercase. Use `|` for union types.

| Convention | Example |
|------------|---------|
| Simple types | `bool`, `u32`, `u64`, `string` |
| Nullable | `u32 \| null`, `string \| null` |
| Enum | `string (enum)` — list values in a separate table below |
| Object | `object \| null` |

### Defaults

- Wrap literal values in backticks: `` `false` ``, `` `true` ``, `` `900` ``
- String defaults include inner quotes: `` `"Moderate"` ``
- Add a human-readable note in parentheses when helpful: `` `86400` (24 hours) ``
- Null defaults: `` `null` `` with explanation if needed: `` `null` (uses the RLDP implementation default) ``

### Enum values

Document enum values in a two-column table directly below the field table:

```markdown
| Value | Description |
|-------|-------------|
| `"Off"` | States are saved synchronously and not cached |
| `"Moderate"` | States are saved asynchronously (recommended) |
```

### JSON examples

- Use ` ```json ` fenced code blocks.
- 2-space indentation.
- Angle brackets for user-supplied values: `<your-external-ip>`, `<dht-private-key-base64>`.
- No comments inside JSON. Explain values in the surrounding text.

### Notes and warnings

Use blockquotes with a bold label:

```markdown
> **Note:** Body text here.
```

For inline emphasis on critical points, use bold: `**Do not disable cells_gc_config.**`

### Cross-references

- Internal anchors: `[field reference](#field-reference)`
- Cross-doc links: `[logging.md](logging.md)` (relative, no path prefix)
- External links: full URL in markdown syntax

### Shell commands

Use ` ```bash ` fenced code blocks. Add a comment if the command needs context:

```bash
# From the repository root
helm install my-node ./helm/ton-rust-node -f values.yaml
```
