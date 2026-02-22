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
echo $(gh auth token) | helm registry login ghcr.io -u $(gh auth status --json login -q .login) --password-stdin
```

### Publishing a new version

1. Update `version` in `Chart.yaml`.
2. Package and push:

```bash
# From the repository root
helm package helm/nodectl
helm push nodectl-<version>.tgz oci://ghcr.io/rsquad/ton-rust-node/helm
```

3. Verify:

```bash
helm show chart oci://ghcr.io/rsquad/ton-rust-node/helm/nodectl --version <version>
```

---

## Documentation guidelines

All documentation lives in the `docs/` directory:

| File | Content |
|------|---------|
| `setup.md` | Step-by-step validator setup guide |
| `elections.md` | Elections, stake policies, SNP, auto-deploy |

### General principles

- **Audience** — operators who deploy and manage TON validators on Kubernetes. Assume familiarity with Helm and containers, but not with TON internals.
- **Be precise** — document what the software actually does. Verify defaults against the source code before writing.
- **Defaults are code defaults** — the "Default" column in field reference tables must show what nodectl uses when the field is omitted.
- **Keep it flat** — prefer tables and short paragraphs over deeply nested prose.
- **Do not duplicate** — link between docs rather than repeating content.

---

## Updating the Parameters table in README

The Parameters section in README.md is auto-generated from `@param` annotations in `values.yaml` using [bitnami/readme-generator-for-helm](https://github.com/bitnami/readme-generator-for-helm).

**Do not edit the Parameters section by hand.**

### Install

```bash
npm install -g @bitnami/readme-generator-for-helm
```

### Run

```bash
readme-generator -v helm/nodectl/values.yaml -r helm/nodectl/README.md
```

### Annotation format

Every documented parameter needs a `## @param` comment in `values.yaml`:

```yaml
## @param port HTTP API port
```

Group parameters with `## @section`:

```yaml
## @section Service parameters
```

Modifiers:

| Modifier | When to use | Example |
|----------|------------|---------|
| `[object]` | Default is `{}` | `## @param probes [object] Probes config` |
| `[array]` | Default is `[]` | `## @param extraEnv [array] Extra env vars` |
| `[nullable]` | Default is `null` | `## @param logFile [nullable] Log file path` |
| `[default: text]` | Custom display text | `## @param config [default: ""] Config JSON` |

Exclude a parameter with `## @skip`:

```yaml
## @skip podDisruptionBudget.maxUnavailable
```

### Workflow

1. Edit `values.yaml` — update parameters and `@param` annotations
2. Run `readme-generator`
3. Verify the generated table
4. Commit both files together

---

## Formatting conventions

### Document structure

1. **H1 title** — one per file
2. **Table of contents** — anchor links to H2 sections
3. **H2 sections** — major topics
4. **H3/H4 subsections** — details

Use `---` between H2 sections for visual separation.

### Field reference tables

| Column | Content |
|--------|---------|
| Field | Dotted path in backticks |
| Type | Lowercase (`bool`, `u32`, `string`, `object`) |
| Required | `yes` or `no` |
| Default | Backtick-wrapped literal (`` `42` ``, `` `"minimum"` ``) |
| Description | Brief explanation |

### JSON examples

- ` ```json ` fenced code blocks
- 2-space indentation
- Angle brackets for user values: `<VAULT_ADDR>`
- No comments inside JSON

### Notes and warnings

```markdown
> **Note:** Body text.
> **Important:** Critical information.
```

### Cross-references

- Internal: `[section name](#anchor)`
- Cross-doc: `[elections.md](elections.md)` (relative)
- Shell commands: ` ```bash ` with context comments
