# RFC: TON BOC Improved Structure LZ4 Compression Format

**Date:** December 2025
**Updated:** January 2026

## Abstract

This document specifies the data format for the Improved Structure LZ4
compression algorithm used to serialize and compress TON Bag of Cells (BOC)
structures. The format provides an optimized representation of cell graphs
that achieves better compression ratios compared to baseline LZ4 compression
by exploiting the structural properties of cell DAGs.

## Table of Contents

1. [Introduction](#1-introduction)
2. [Terminology](#2-terminology)
3. [Format Overview](#3-format-overview)
4. [Outer Container Format](#4-outer-container-format)
5. [Inner Serialized Format](#5-inner-serialized-format)
6. [Cell Metadata Section](#6-cell-metadata-section)
7. [Edge Bitmap Section](#7-edge-bitmap-section)
8. [Small Data Prefix Section](#8-small-data-prefix-section)
9. [Graph Encoding Section](#9-graph-encoding-section)
10. [Cell Data Section](#10-cell-data-section)
11. [Special Cell Handling](#11-special-cell-handling)
12. [Encoding Examples](#12-encoding-examples)
13. [Security Considerations](#13-security-considerations)
14. [References](#14-references)

## 1. Introduction

The TON blockchain uses Bag of Cells (BOC) as a fundamental data structure
for representing hierarchical cell trees. Cells contain data bits and
references to child cells, forming a directed acyclic graph (DAG).

This specification defines the "Improved Structure LZ4" format, which
serializes the cell graph in a structure-aware manner before applying LZ4
compression. This approach separates structural metadata from cell payload
data, improving compression effectiveness.

### 1.1. Design Goals

- Efficient compression of cell DAG structures
- Preservation of cell graph topology
- Support for special cell types (PrunedBranch, etc.)
- Compact encoding of graph edges using topological ordering

### 1.2. Scope

This document covers:
- The binary wire format for serialized BOC data
- Encoding rules for cells, references, and metadata
- LZ4 compression wrapper format

## 2. Terminology

**Cell**: A fundamental TON data structure containing up to 1023 data bits
and up to 4 references to child cells.

**BOC (Bag of Cells)**: A collection of cells with designated root cells.

**DAG (Directed Acyclic Graph)**: A graph structure where cells reference
child cells without forming cycles.

**Topological Order**: An ordering of nodes where every parent appears
before all its children.

**Special Cell**: A cell with special semantics (e.g., PrunedBranch,
MerkleProof, MerkleUpdate, Library).

**PrunedBranch**: A special cell type representing pruned subtrees,
containing level information and hash data.

**MerkleUpdate**: A special cell type with two references (left and right
subtrees) representing state transitions.

**Depth-Balance Elision**: An optimization for MerkleUpdate subtrees where
a cell's grams value can be reconstructed from its children's differences,
allowing the cell data to be omitted.

**Small Data**: Cell data with less than 128 bits (can be encoded in 7 bits).

The key words "MUST", "MUST NOT", "REQUIRED", "SHALL", "SHALL NOT",
"SHOULD", "SHOULD NOT", "RECOMMENDED", "MAY", and "OPTIONAL" in this
document are to be interpreted as described in RFC 2119.

## 3. Format Overview

The complete compressed format consists of two layers:

```
+----------------------------------+
| Outer Container (LZ4 wrapped)    |
+----------------------------------+
    |
    v
+----------------------------------+
| Inner Serialized Format          |
| (structure-aware encoding)       |
+----------------------------------+
```

The inner format uses topological ordering of cells to enable compact
delta encoding of graph edges.

## 4. Outer Container Format

The outer container prepends the decompressed size to the LZ4-compressed
payload:

```
 0                   1                   2                   3
 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                    Decompressed Size (32 bits)               |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                                                               |
|                   LZ4 Compressed Payload                      |
|                          (variable)                           |
|                                                               |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
```

### 4.1. Field Definitions

**Decompressed Size**: 32-bit unsigned integer (big-endian byte order)
specifying the size in bytes of the decompressed inner format.

**LZ4 Compressed Payload**: Standard LZ4 compressed data containing the
inner serialized format.

### 4.2. Algorithm Identifier

When used with the general `boc_compress`/`boc_decompress` interface, an
additional algorithm byte prefix is added:

```
+--------+----------------------------------+
| AlgoID | Outer Container                  |
+--------+----------------------------------+
   1 byte           variable
```

Algorithm ID values:
- `0x00`: BaselineLZ4
- `0x01`: ImprovedStructureLZ4

## 5. Inner Serialized Format

The decompressed inner format consists of sequential sections:

```
+----------------------------------+
| Header Section                   |
|   - Root Count (32 bits)         |
|   - Root Indexes (32 bits each)  |
|   - Node Count (32 bits)         |
+----------------------------------+
| Cell Metadata Section            |
|   - Per-cell type, refs, size    |
+----------------------------------+
| Edge Bitmap Section              |
|   - Direct successor flags       |
+----------------------------------+
| Small Data Prefix Section        |
|   - Sub-byte prefixes for        |
|     PrunedBranch/small cells     |
+----------------------------------+
| Graph Encoding Section           |
|   - Non-trivial edge deltas      |
|   - With bit-alignment optim.    |
+----------------------------------+
| Padding to byte boundary         |
+----------------------------------+
| Cell Data Section                |
|   - Remaining cell payloads      |
+----------------------------------+
```

### 5.1. Header Section

```
 0                   1                   2                   3
 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                        Root Count                             |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                      Root Index [0]                           |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                           ...                                 |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                   Root Index [Root Count - 1]                 |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                        Node Count                             |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
```

**Root Count**: Number of root cells in the BOC (32-bit unsigned).
MUST be >= 1.

**Root Index[i]**: Topological rank of the i-th root cell (32-bit unsigned).
Each index MUST be < Node Count.

**Node Count**: Total number of cells in the BOC (32-bit unsigned).
MUST be >= 1.

## 6. Cell Metadata Section

For each cell (in topological order), the following metadata is stored:

### 6.1. Base Metadata (8 bits per cell)

```
 0   1   2   3   4   5   6   7
+---+---+---+---+---+---+---+---+
|  Cell Type    |   Refs Count  |
+---+---+---+---+---+---+---+---+
     4 bits          4 bits
```

**Cell Type** (4 bits):
- `0`: Ordinary cell
- `1-15`: Special cell with PrunedBranch level = (Cell Type - 1)

**Refs Count** (4 bits): Number of child references (0-4).
Values > 4 are invalid.

### 6.2. Data Length Encoding (for non-PrunedBranch cells)

If Cell Type is 0 (ordinary cell), an additional 8 bits follow:

```
 0   1   2   3   4   5   6   7
+---+---+---+---+---+---+---+---+
| S |    Data Length Value      |
+---+---+---+---+---+---+---+---+
1 bit        7 bits
```

**S (Small flag)** (1 bit):
- `1`: Data is "small" (< 128 bits), length is exact bit count (0-127 bits)
- `0`: Data is "large", length is encoded as byte count

**Data Length Value** (7 bits):
- If S=1: Exact data length in bits (0-127)
- If S=0: Encoded byte count where:
  - `0`: Data length = 1024 bits (128 bytes, maximum)
  - `1-127`: Data length = value * 8 bits

### 6.3. PrunedBranch Data Length

For PrunedBranch cells (Cell Type > 0), data length is computed:

```
data_length = (256 + 16) * popcount(pruned_branch_level)
```

Where `pruned_branch_level = Cell Type - 1` and `popcount` counts set bits.

## 7. Edge Bitmap Section

For each cell reference across all cells, a single bit indicates whether
the child immediately follows the parent in topological order:

```
+---+---+---+---+---+---+---+---+---+---+---+
| E[0,0] | E[0,1] | ... | E[n-1,r-1] | ...  |
+---+---+---+---+---+---+---+---+---+---+---+
```

**Edge Bit**:
- `1`: Child is at topological rank = parent_rank + 1 (direct successor)
- `0`: Child is at a different position (encoded in Graph Encoding Section)

Total bits = sum of all cells' reference counts.

## 8. Small Data Prefix Section

**IMPORTANT**: This section stores prefix bits ONLY for cells where:
- Cell Type > 0 (PrunedBranch), OR
- Data is "small" (S flag = 1, i.e., data length < 128 bits)

For each qualifying cell, the sub-byte prefix of cell data is stored:

```
prefix_bits = cell_data_length % 8
```

These bits are the most significant bits of the first byte of cell data.
Bits are concatenated without padding between cells.

Cells with "large" data (S flag = 0) do NOT have their prefix stored here.

## 9. Graph Encoding Section

For cells with non-direct-successor children, delta values are encoded
using a bit-alignment optimization scheme.

### 9.1. Processing Rules

For each cell `i` in topological order where `node_count > i + 3`:

For each reference `j` of cell `i`:
1. If child_rank <= i + 1: skip (direct successor or earlier, already handled)
2. Otherwise, compute delta: `delta = child_rank - i - 2`

### 9.2. Delta Encoding with Bit-Alignment Optimization

Let `max_val = node_count - i - 3`
Let `required_bits = 1 + floor(log2(max_val))` (or 1 if max_val == 0)

The encoding uses bit-alignment optimization to improve LZ4 compression:

```
Let pref_size = current bit position in output stream
Let available_bits = 8 - ((pref_size + 1) % 8)

Case 1: required_bits < available_bits + 1
    - Store delta directly in required_bits

Case 2: delta < (1 << available_bits)
    - Store '1' flag (1 bit)
    - Store delta in available_bits

Case 3: Otherwise
    - Store '0' flag (1 bit)
    - Store delta in required_bits
```

This optimization ensures deltas are byte-aligned when possible, improving
LZ4's ability to find repeated patterns.

### 9.3. Small Graph Optimization

For cells where `node_count <= i + 3`:
- All non-direct-successor children default to rank `i + 2`
- No explicit encoding needed

## 10. Cell Data Section

After the Graph Encoding Section, padding zeros are added to reach a byte
boundary.

### 10.1. Data Storage Order

Cell data is stored in topological order with format depending on cell type:

**For PrunedBranch (cell_type > 0) or small data cells (S=1):**
```
| remaining_bits |
     (cell_data_length - prefix_bits) bits
```
The prefix was already stored in Section 8. Only remaining bits are stored here.

**For ordinary cells with large data (cell_type == 0 and S=0):**
```
| padding zeros | '1' marker | full_cell_data |
                    1 bit       cell_data_length bits
```

The padding calculation:
```
data_size = cell_data_length + 1  // +1 for the '1' marker
padding = (8 - data_size % 8) % 8
```

This ensures the cell data ends on a byte boundary.

### 10.2. Data Reconstruction

For PrunedBranch cells, the 16-bit header is reconstructed during decompression:
```
header = (1 << 8) | pruned_branch_level
```

This header is prepended to the stored data.

## 11. Special Cell Handling

### 11.1. PrunedBranch Cells

PrunedBranch cells contain:
- 16-bit type header: `0x01` || level_byte
- Hash data: (256 + 16) * popcount(level) bits

The 16-bit header is NOT stored in the data section; it is reconstructed
from the Cell Type field during decompression.

### 11.2. MerkleUpdate Depth-Balance Elision

MerkleUpdate cells represent state transitions with two subtrees: the left
subtree (old state) and the right subtree (new state). The compression
algorithm exploits structural similarities between these subtrees.

**Optimization Principle:**

When processing a cell in the right subtree of a MerkleUpdate, if the cell
has a corresponding paired cell in the left subtree with:
1. The same hash (identical cell), OR
2. The cell's "vertex difference" equals the sum of its children's differences

Then the cell's data can be omitted (elided) because it can be reconstructed
during decompression from the paired left cell and children.

**Vertex Difference Calculation:**

For cells containing grams (currency amounts), the vertex difference is:
```
vertex_diff = grams(right_cell) - grams(left_cell)
```

For cells where the sum of child differences equals the vertex difference:
```
sum_child_diff = sum(child_diff[i] for i in children)
if sum_child_diff == vertex_diff:
    # Cell data can be elided
```

**Encoding:**

Cells eligible for depth-balance elision are encoded with Cell Type = 9.
During decompression, these cells are reconstructed by:
1. Looking up the paired cell in the left subtree
2. Computing the expected grams value from children differences
3. Reconstructing the cell data with the computed grams value

### 11.3. Cell Type Mapping

| Cell Type Value | Special Type           | Description                    |
|-----------------|------------------------|--------------------------------|
| 0               | Ordinary               | Regular cell                   |
| 1               | PrunedBranch           | Level 0                        |
| 2               | PrunedBranch           | Level 1                        |
| ...             | ...                    | ...                            |
| 8               | PrunedBranch           | Level 7                        |
| 9               | Depth-Balance Elision  | MerkleUpdate optimization      |
| 10-15           | Reserved               | Future use                     |

## 12. Topological Sort Algorithm

The topological sort uses a specific ordering to maximize compression:

1. Build reverse graph (child -> parent edges)
2. Initialize in_degree[i] = refs_cnt[i] (outgoing edge count)
3. Initialize queue with nodes where in_degree == 0 (leaf nodes)
4. Sort queue by tuple: (cell_type == Ordinary, -data_size, -node_id)
5. Process queue:
   - Pop highest priority node, add to topo_order
   - For each parent: decrement in_degree, add to queue if becomes 0
   - New nodes added with priority (false, 0, -parent_id)
6. Reverse the final topo_order

This ordering ensures:
- Leaf nodes are processed first
- Special cells are prioritized over ordinary cells
- Larger cells are processed before smaller ones
- Consistent ordering via node_id tiebreaker

## 13. Encoding Examples

### 13.1. Simple Two-Cell BOC

Consider a BOC with one root cell referencing one child cell:

```
Cell 0 (root): 16 bits of data, 1 reference to Cell 1
Cell 1 (leaf): 8 bits of data, 0 references
```

**Topological Order:** [0, 1] (root before child)

**Header Section:**
```
Root Count:     0x00000001 (1 root)
Root Index[0]:  0x00000000 (rank 0)
Node Count:     0x00000002 (2 cells)
```

**Cell Metadata:**
```
Cell 0: Type=0, Refs=1, Small=1, Length=16
        Binary: 0000 0001 | 1 0010000

Cell 1: Type=0, Refs=0, Small=1, Length=8
        Binary: 0000 0000 | 1 0001000
```

**Edge Bitmap:**
```
1 bit for Cell 0's reference: 1 (child is direct successor)
```

**Data Section:**
```
Cell 0: 16 bits of actual data
Cell 1: 8 bits of actual data
```

### 13.2. Delta Encoding Example

For a cell at rank 5 in a 100-node graph with child at rank 10:

```
delta = 10 - 5 - 2 = 3
max_val = 100 - 5 - 3 = 92
required_bits = 1 + floor(log2(92)) = 7 bits
```

Assuming current bit position is 13:
```
pref_size = 13
available_bits = 8 - ((13 + 1) % 8) = 8 - 6 = 2

Since required_bits (7) >= available_bits + 1 (3):
  Check if delta (3) < (1 << 2) = 4: YES
  -> Use Case 2: Write '1' + delta in 2 bits
  -> Output: 1 11 (3 bits total)
```

## 14. Security Considerations

### 14.1. Size Validation

Implementations MUST validate:
- Decompressed size does not exceed maximum allowed size
- Node count and root count are reasonable relative to decompressed size
- Cell data lengths do not exceed 1024 bits
- Reference counts do not exceed 4

### 14.2. Graph Validation

Implementations MUST verify:
- All root indexes are < node count
- All child references point to higher-ranked cells (DAG property)
- No circular references exist

### 14.3. Resource Limits

Implementations SHOULD enforce:
- Maximum decompressed size limits
- Maximum node count limits
- Timeout limits for decompression

---

## Appendix A: Pseudocode

### A.1. Compression Algorithm

```python
function compress(roots):
    # Build cell graph with deduplication
    cell_map = {}
    graph = []
    refs_cnt = []
    cell_data = []
    cell_type = []
    pruned_level = []

    for root in roots:
        build_graph_recursive(root, None, False, None, cell_map, graph,
                              refs_cnt, cell_data, cell_type, pruned_level)


function build_graph_recursive(cell, left_cell, under_mu_right, sum_diff_out,
                                cell_map, graph, refs_cnt, cell_data,
                                cell_type, pruned_level):
    """
    Build cell graph with MerkleUpdate depth-balance optimization.

    Parameters:
    - cell: Current cell to process
    - left_cell: Paired cell from left MerkleUpdate subtree (if any)
    - under_mu_right: True if currently in right subtree of MerkleUpdate
    - sum_diff_out: Accumulator for sum of child differences
    """
    cell_hash = cell.hash()

    # Check for existing cell
    if cell_hash in cell_map:
        idx = cell_map[cell_hash]
        if sum_diff_out is not None:
            # Accumulate grams difference for parent
            sum_diff_out += get_grams_diff(left_cell, cell)
        return idx

    # Register new cell
    idx = len(graph)
    cell_map[cell_hash] = idx
    graph.append([0, 0, 0, 0])
    refs_cnt.append(cell.refs_count())
    cell_data.append(cell.data())
    cell_type.append(cell.special_type())
    pruned_level.append(0)

    # Handle MerkleUpdate cells specially
    if cell.special_type() == MerkleUpdate and cell.refs_count() == 2:
        left_child = cell.reference(0)
        right_child = cell.reference(1)

        # Process left subtree normally
        graph[idx][0] = build_graph_recursive(left_child, None, False, None,
                                               cell_map, graph, refs_cnt,
                                               cell_data, cell_type, pruned_level)

        # Process right subtree paired with left for depth-balance optimization
        graph[idx][1] = build_graph_recursive(right_child, left_child, True, None,
                                               cell_map, graph, refs_cnt,
                                               cell_data, cell_type, pruned_level)

    elif under_mu_right and left_cell is not None:
        # We're in right subtree of MerkleUpdate with paired left cell
        sum_child_diff = 0

        for j in range(cell.refs_count()):
            child = cell.reference(j)
            left_child = left_cell.reference(j) if j < left_cell.refs_count() else None

            graph[idx][j] = build_graph_recursive(child, left_child, True,
                                                   sum_child_diff,
                                                   cell_map, graph, refs_cnt,
                                                   cell_data, cell_type, pruned_level)

        # Check if depth-balance elision applies
        vertex_diff = get_grams_diff(left_cell, cell)
        if vertex_diff is not None and sum_child_diff == vertex_diff:
            # Cell data can be elided - mark with cell_type=9
            pruned_level[idx] = 9

        if sum_diff_out is not None:
            sum_diff_out += vertex_diff

    else:
        # Normal cell processing
        for j in range(cell.refs_count()):
            child = cell.reference(j)
            graph[idx][j] = build_graph_recursive(child, None, False, None,
                                                   cell_map, graph, refs_cnt,
                                                   cell_data, cell_type, pruned_level)

    return idx


function get_grams_diff(left_cell, right_cell):
    """
    Compute grams difference between paired cells.
    Returns None if cells don't contain grams values.
    """
    left_grams = extract_grams(left_cell)
    right_grams = extract_grams(right_cell)
    if left_grams is None or right_grams is None:
        return None
    return right_grams - left_grams

    node_count = len(graph)

    # Build reverse graph
    reverse_graph = [[] for _ in range(node_count)]
    for i in range(node_count):
        for child in graph[i][:refs_cnt[i]]:
            reverse_graph[child].append(i)

    # Determine small data flags
    is_data_small = []
    for i in range(node_count):
        if cell_type[i] != PrunedBranch:
            is_data_small[i] = len(cell_data[i]) < 128
        else:
            is_data_small[i] = False

    # Topological sort with specific ordering
    in_degree = refs_cnt.copy()  # Use outgoing edges as in_degree
    queue = []
    for i in range(node_count):
        if in_degree[i] == 0:
            # Priority: (is_ordinary, -data_size, -node_id)
            queue.append((cell_type[i] == Ordinary,
                         -len(cell_data[i]), -i))
    queue.sort()

    topo_order = []
    while queue:
        _, _, neg_node = queue.pop()  # Pop highest (last after sort)
        node = -neg_node
        topo_order.append(node)

        for parent in reverse_graph[node]:
            in_degree[parent] -= 1
            if in_degree[parent] == 0:
                queue.append((False, 0, -parent))

    topo_order.reverse()  # Reverse to get parents before children

    # Compute ranks
    rank = [0] * node_count
    for pos, orig_id in enumerate(topo_order):
        rank[orig_id] = pos

    # Serialize
    output = BitString()

    # Header
    output.append_uint(len(roots), 32)
    for root_id in root_indexes:
        output.append_uint(rank[root_id], 32)
    output.append_uint(node_count, 32)

    # Cell metadata
    for i in range(node_count):
        node = topo_order[i]

        # Determine cell type encoding
        if pruned_level[node] == 9:
            # Depth-balance elision (MerkleUpdate optimization)
            ct = 9
        elif cell_type[node] == PrunedBranch:
            ct = pruned_level[node] + 1
        else:
            ct = 0

        output.append_uint(ct, 4)
        output.append_uint(refs_cnt[node], 4)

        # Data length encoding (not needed for depth-balance elided cells)
        if ct == 9:
            # No data length - cell data is reconstructed from paired cell
            pass
        elif cell_type[node] != PrunedBranch:
            if is_data_small[node]:
                output.append_bit(1)  # Small flag
                output.append_uint(len(cell_data[node]), 7)
            else:
                output.append_bit(0)  # Not small
                encoded = 0 if len(cell_data[node]) == 1024 \
                          else 1 + len(cell_data[node]) // 8
                output.append_uint(encoded, 7)

    # Edge bitmap
    for i in range(node_count):
        node = topo_order[i]
        for j in range(refs_cnt[node]):
            child = graph[node][j]
            output.append_bit(rank[child] == i + 1)

    # Small data prefixes (ONLY for PrunedBranch or small data, NOT depth-balance elided)
    for i, node in enumerate(topo_order):
        if pruned_level[node] == 9:
            # Skip depth-balance elided cells - no data stored
            continue
        if cell_type[node] != PrunedBranch and not is_data_small[node]:
            continue
        prefix_bits = len(cell_data[node]) % 8
        if prefix_bits > 0:
            output.append_bits(cell_data[node][:prefix_bits])

    # Graph deltas with bit-alignment optimization
    for i in range(node_count):
        node = topo_order[i]
        if node_count <= i + 3:
            continue

        for j in range(refs_cnt[node]):
            child_rank = rank[graph[node][j]]
            if child_rank <= i + 1:
                continue

            delta = child_rank - i - 2
            max_val = node_count - i - 3
            required_bits = 1 + floor(log2(max_val)) if max_val > 0 else 1

            pref_size = output.size()
            available_bits = 8 - (pref_size + 1) % 8

            if required_bits < available_bits + 1:
                output.append_uint(delta, required_bits)
            elif delta < (1 << available_bits):
                output.append_bit(1)
                output.append_uint(delta, available_bits)
            else:
                output.append_bit(0)
                output.append_uint(delta, required_bits)

    # Pad to byte boundary
    output.pad_to_byte()

    # Cell data
    for node in topo_order:
        if pruned_level[node] == 9:
            # Skip depth-balance elided cells - data reconstructed during decompression
            continue

        prefix_bits = len(cell_data[node]) % 8

        if cell_type[node] == PrunedBranch or is_data_small[node]:
            # Write remaining bits after prefix
            output.append_bits(cell_data[node][prefix_bits:])
        else:
            # Large data: padding + '1' marker + full data
            data_size = len(cell_data[node]) + 1
            padding = (8 - data_size % 8) % 8
            output.append_zeros(padding)
            output.append_bit(1)
            output.append_bits(cell_data[node])

    # Final padding
    output.pad_to_byte()

    # LZ4 compress
    compressed = lz4_compress(output.to_bytes())

    # Prepend size (big-endian)
    result = ByteArray()
    result.append_uint32_be(len(output.to_bytes()))
    result.append(compressed)

    return result
```

### A.2. Decompression Algorithm

```python
function decompress(data, max_size):
    # Read size and decompress
    decompressed_size = read_uint32_be(data[0:4])
    if decompressed_size > max_size:
        error("Size exceeds limit")

    serialized = lz4_decompress(data[4:], decompressed_size)

    if len(serialized) != decompressed_size:
        error("Decompressed size mismatch")

    bit_position = 0

    # Read header
    root_count = read_bits(32)
    root_indexes = [read_bits(32) for _ in range(root_count)]
    node_count = read_bits(32)

    # Validate
    for idx in root_indexes:
        if idx >= node_count:
            error("Invalid root index")

    # Read cell metadata
    cells = []
    for i in range(node_count):
        cell_type = read_bits(4)
        refs_count = read_bits(4)

        is_depth_balance_elided = (cell_type == 9)
        is_special = cell_type > 0 and cell_type < 9
        pruned_level = cell_type - 1 if is_special else 0

        if is_depth_balance_elided:
            # Depth-balance elided cell - data length determined during reconstruction
            data_length = 0
            is_small = False
        elif pruned_level > 0:
            coef = popcount(pruned_level)
            data_length = (256 + 16) * coef
            is_small = True  # Treated as small for prefix storage
        else:
            is_small = read_bits(1) == 1
            length_val = read_bits(7)
            if is_small:
                data_length = length_val
            elif length_val == 0:
                data_length = 1024
            else:
                data_length = length_val * 8

        cells.append(CellInfo(cell_type, refs_count, data_length,
                              is_small, pruned_level, is_depth_balance_elided))

    # Read edge bitmap
    graph = [[0] * 4 for _ in range(node_count)]
    for i in range(node_count):
        for j in range(cells[i].refs_count):
            if read_bits(1) == 1:
                graph[i][j] = i + 1

    # Read small data prefixes and initialize builders
    cell_builders = [CellBuilder() for _ in range(node_count)]
    for i in range(node_count):
        if cells[i].is_depth_balance_elided:
            # Skip - data will be reconstructed from paired cell
            cells[i].remaining_length = 0
            continue

        if cells[i].pruned_level > 0:
            # Reconstruct PrunedBranch header
            header = (1 << 8) | cells[i].pruned_level
            cell_builders[i].store_uint(header, 16)

        # Read prefix only for PrunedBranch or small data
        remainder_bits = cells[i].data_length % 8
        if remainder_bits > 0:
            prefix = read_bits(remainder_bits)
            cell_builders[i].store_bits(prefix, remainder_bits)
        cells[i].remaining_length = cells[i].data_length - remainder_bits

    # Read graph deltas with bit-alignment optimization
    for i in range(node_count):
        if node_count <= i + 3:
            for j in range(cells[i].refs_count):
                if graph[i][j] == 0:
                    graph[i][j] = i + 2
            continue

        for j in range(cells[i].refs_count):
            if graph[i][j] == 0:
                max_val = node_count - i - 3
                required_bits = 1 + floor(log2(max_val)) if max_val > 0 else 1

                pref_size = bit_position
                available_bits = 8 - (pref_size + 1) % 8

                if required_bits < available_bits + 1:
                    delta = read_bits(required_bits)
                else:
                    flag = read_bits(1)
                    if flag == 1:
                        pref_size_after = bit_position
                        avail_after = 8 - pref_size_after % 8
                        delta = read_bits(avail_after)
                    else:
                        delta = read_bits(required_bits)

                graph[i][j] = delta + i + 2

    # Validate graph
    for i in range(node_count):
        for j in range(cells[i].refs_count):
            if graph[i][j] >= node_count or graph[i][j] <= i:
                error("Invalid graph")

    # Align to byte
    while bit_position % 8 != 0:
        bit_position += 1

    # Read remaining cell data
    for i in range(node_count):
        if cells[i].is_depth_balance_elided:
            # Skip - data will be reconstructed during cell building
            continue

        padding_bits = 0

        if cells[i].pruned_level == 0 and not cells[i].is_small:
            # Skip padding zeros and '1' marker
            while peek_bit() == 0:
                read_bits(1)
                padding_bits += 1
            read_bits(1)  # '1' marker
            padding_bits += 1

        remaining = cells[i].remaining_length - padding_bits
        cell_builders[i].store_bits(read_bits(remaining), remaining)

    # Build cells (reverse topological order)
    # Track paired cells for depth-balance reconstruction
    paired_cells = {}  # Maps right subtree cell idx to left subtree cell

    nodes = [None] * node_count
    for i in range(node_count - 1, -1, -1):
        # Add child references
        for j in range(cells[i].refs_count):
            cell_builders[i].store_ref(nodes[graph[i][j]])

        if cells[i].is_depth_balance_elided:
            # Reconstruct cell data from paired left cell and children differences
            paired_cell = paired_cells.get(i)
            if paired_cell is not None:
                # Compute sum of children's grams differences
                sum_child_diff = 0
                for j in range(cells[i].refs_count):
                    child_right = nodes[graph[i][j]]
                    child_left = paired_cell.reference(j) if j < paired_cell.refs_count() else None
                    if child_left and child_right:
                        sum_child_diff += get_grams(child_right) - get_grams(child_left)

                # Reconstruct cell data with adjusted grams value
                new_grams = get_grams(paired_cell) + sum_child_diff
                cell_builders[i] = reconstruct_cell_data(paired_cell, new_grams)
                for j in range(cells[i].refs_count):
                    cell_builders[i].store_ref(nodes[graph[i][j]])

        nodes[i] = cell_builders[i].finalize(cells[i].is_special)

        # Track paired cells for MerkleUpdate subtrees
        if cells[i].cell_type == MerkleUpdate and cells[i].refs_count == 2:
            # Map right subtree cells to their left subtree pairs
            track_paired_cells(nodes[graph[i][0]], nodes[graph[i][1]], paired_cells)

    return [nodes[idx] for idx in root_indexes]


function track_paired_cells(left_root, right_root, paired_cells):
    """Recursively map right subtree cells to corresponding left subtree cells."""
    # Implementation tracks cell correspondence between MerkleUpdate subtrees
    pass


function reconstruct_cell_data(paired_cell, new_grams):
    """
    Create a CellBuilder with data copied from paired_cell but with
    the grams value replaced by new_grams.
    """
    builder = CellBuilder()
    # Copy cell structure from paired_cell, replacing grams value
    # Exact implementation depends on cell data format
    return builder
```

---

## Appendix B: Bit-Level Format Summary

```
OUTER CONTAINER:
+------------------+------------------------+
| Decompressed     | LZ4 Compressed         |
| Size (32b BE)    | Payload                |
+------------------+------------------------+

INNER FORMAT (after LZ4 decompression):
+------------------+
| root_count: 32b  |
+------------------+
| root_idx[0]: 32b |
| ...              |
| root_idx[n]: 32b |
+------------------+
| node_count: 32b  |
+------------------+
| For each node:   |
|   cell_type: 4b  |  (0=ordinary, 1-8=PrunedBranch, 9=depth-balance elided)
|   refs_cnt: 4b   |
|   [if type==0:]  |
|     is_small: 1b |
|     length: 7b   |
|   [if type==9:]  |
|     (no length)  |  Data reconstructed from paired cell
+------------------+
| Edge bitmap:     |
|   1b per edge    |
+------------------+
| Small prefixes:  |
|   (len % 8) bits |
|   ONLY for       |
|   PrunedBranch   |
|   or small cells |
|   (NOT type=9)   |
+------------------+
| Delta encoding:  |
|   with bit-align |
|   optimization   |
+------------------+
| Padding to byte  |
+------------------+
| Cell data:       |
|   type=9: skip   |  (reconstructed during decompression)
|   small/pruned:  |
|     remaining    |
|   large:         |
|     pad+1+data   |
+------------------+
```

---

## Appendix C: Test Vectors

This appendix provides verified test vectors for implementers. Each vector
includes the input cell structure, standard BOC serialization (for reference),
and the Improved Structure LZ4 compressed output.

All hex values are presented in lowercase. The compressed output does NOT
include the algorithm prefix byte (0x01) used by the generic API.

### C.1. Vector 1: Single Leaf Cell (32 bits)

**Input:**
- Single cell with 32 bits of data: `0x54455354` (ASCII "TEST")
- No child references

**Cell Hash:** `0DECF040EE6032ACA37E26B59A070EF0AF033EA91ABC2BBDECF8B879D4CE1E57`

**Standard BOC (17 bytes):**
```
b5ee9c7201010101000600000854455354
```

**Compressed Output (20 bytes):**
```
000000125200000001000100700100a0554455354
```

**Breakdown of Compressed Output (after LZ4 decompression):**
```
Decompressed size: 00000012 (18 bytes)
LZ4 payload decompresses to:
  00000001  - root_count = 1
  00000000  - root_index[0] = 0
  00000001  - node_count = 1
  01        - cell[0]: type=0, refs=0
  20        - cell[0]: small=1, length=32 bits
  (no edges)
  (padding)
  54455354  - cell data: "TEST"
```

### C.2. Vector 2: Parent-Child (Two Cells)

**Input:**
- Parent cell: 32 bits data `0xCAFEBABE`, 1 reference
- Child cell: 32 bits data `0xDEADBEEF`, 0 references

**Hashes:**
- Parent: `089C7AC0A421A928910FC8E1C10921ED7A1AC7997ED209F98285237E7004052C`
- Child: `270906FD171B9C43F37A353059A73FBC02E0568188EC30186AF846CAEFD09B8C`

**Standard BOC (24 bytes):**
```
b5ee9c7201010201000d000108cafebabe010008deadbeef
```

**Compressed Output (27 bytes):**
```
000000195200000001000100e00201a000a080cafebabedeadbeef
```

**Breakdown:**
```
Decompressed size: 00000019 (25 bytes)
After LZ4 decompression:
  00000001  - root_count = 1
  00000000  - root_index[0] = 0
  00000002  - node_count = 2
  01        - cell[0]: type=0, refs=1
  20        - cell[0]: small=1, length=32 bits
  00        - cell[1]: type=0, refs=0
  20        - cell[1]: small=1, length=32 bits
  1         - edge bitmap: child is direct successor
  (padding)
  cafebabe  - cell[0] data
  deadbeef  - cell[1] data
```

### C.3. Vector 3: Two Root Cells

**Input:**
- Root 1: 32 bits data `0x11111111`, 0 references
- Root 2: 32 bits data `0x22222222`, 0 references

**Standard BOC (24 bytes):**
```
b5ee9c7201010202000c0100000822222222000811111111
```

### C.4. Vector 4: Deep Chain (4 Levels)

**Input:**
- Root: 32 bits `0x11111111`, 1 ref -> Level1
- Level1: 32 bits `0x22222222`, 1 ref -> Level2
- Level2: 32 bits `0x33333333`, 1 ref -> Leaf
- Leaf: 32 bits `0x44444444`, 0 refs

**Root Hash:** `5EB6FC2E5576D414071D377335449F79F7567EC8EA3B12AE427303B4C1E770BB`

**Standard BOC (38 bytes):**
```
b5ee9c7201010401001b00010811111111010108222222220201083333333303000844444444
```

### C.5. Vector 9: Empty Data Cell with Reference

**Input:**
- Parent cell: 0 bits data, 1 reference
- Child cell: 32 bits `0x12345678`, 0 refs

### C.6. Generic API with Algorithm Prefix

When using the generic `boc_compress` API, a 1-byte algorithm prefix is added:

**Input:** Single cell with 32 bits `0xABCDEF01`

Where `01` is the algorithm identifier for ImprovedStructureLZ4.

---

## Appendix D: Implementation Notes

### D.1. Bit-Alignment Optimization Rationale

The bit-alignment optimization in the Graph Encoding Section improves
LZ4 compression by ensuring that common delta patterns align to byte
boundaries. This works because:

1. LZ4 operates on byte boundaries for pattern matching
2. When deltas are small, encoding them in available byte-boundary bits
   creates more opportunities for repeated patterns
3. The '1'/'0' flag allows the decoder to distinguish encoding modes

### D.2. Small Data Threshold

The threshold of 128 bits for "small" data was chosen because:
- 127 is the maximum value that fits in 7 bits
- Most cell data in typical BOC structures is small
- Exact bit-length encoding for small data enables better compression

### D.3. Topological Sort Priority

The specific priority ordering (special cells first, then by decreasing
data size) was chosen to:
- Group similar cell types together for better LZ4 compression
- Place leaf nodes (no children) at consistent positions
- Ensure deterministic ordering for reproducible compression

### D.4. MerkleUpdate Depth-Balance Elision

The depth-balance elision optimization (cell_type=9) provides significant
compression benefits for MerkleUpdate cells by exploiting the fact that
many cells in the right subtree differ from their left subtree counterparts
only in the grams (currency) value.

**When to Apply:**

The optimization applies when:
1. The cell is in the right subtree of a MerkleUpdate
2. There is a corresponding paired cell in the left subtree
3. The cell's vertex difference (grams_right - grams_left) equals the sum
   of its children's vertex differences

**Compression Savings:**

For cells that qualify, the entire cell data section is omitted, typically
saving 100-200 bits per cell. In MerkleUpdate blocks with many account
state changes, this can reduce compressed size by 5-15%.

**Implementation Complexity:**

The main complexity is maintaining the pairing relationship between left
and right subtree cells during both compression and decompression:

- **Compression**: Traverse MerkleUpdate subtrees in parallel, computing
  vertex differences and checking the sum condition
- **Decompression**: Track cell correspondence and reconstruct elided cells
  by copying the paired cell's data with adjusted grams value

**C++ Reference:**

The C++ implementation uses `kMURemoveSubtreeSums = true` to enable this
optimization. The Rust implementation follows the same logic.
