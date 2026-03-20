# Accelerated Consensus Optimizations

This document describes the key optimizations implemented in the accelerated consensus protocol to improve performance and reduce latency in the TON validator network.

## Rarely Rotating Collator

The rarely rotating collator optimization introduces a dynamic collator selection mechanism that maintains stability while ensuring performance.

### Key Concepts

- **Deterministic Random Selection**: The collator index is chosen using a deterministic random approach, providing predictability while maintaining fairness across validators. This is needed to avoid the case when validator with index 0 becomes collator simultaneously in several shards and is overloaded; so deterministic random allows to balance network load between collators.

- **Performance-Based Rotation**: Collator changes occur only when bad performance is detected, specifically after N sequential skip rounds (skip commits). The reason for skipping doesn't matter - whether it's collation timeout, collation error, or absence of 2/3 approvals for the block-candidate.

### Consensus Flow

Each consensus round can finish with one of two outcomes:
1. **Successful Commit**: Block-candidate from the current collator is committed
2. **Skip Round**: Empty block is committed

### Simplified BFT Protocol

To accelerate consensus, several phases from the original BFT consensus implementation have been removed:
- ~~VOTING phase~~
- ~~PRECOMMIT phase~~

The consensus now operates with a streamlined 3-step process:

#### 1. GENERATION
The collator generates a block-candidate for the current round.

#### 2. APPROVALS
Each node in a round may approve **ONLY ONE** block:
- Block-candidate from collator, OR
- EMPTY block

**Approval Thresholds**:
- **Block-candidate approval**: Requires more than 2/3 of validator weights
- **Empty block approval**: Requires more than 1/3 of validator weights
  - When 1/3+ approve empty block, it becomes impossible for block-candidate to achieve 2/3 approval

#### 3. COMMIT
The approved block (either block-candidate or empty) is committed to the chain.

### Implementation Details

- **Round State Integration**: Most changes are implemented as special cases within the existing round state implementation
- **Dynamic Collator Priority**: The collator index for each round is computed in the state implementation based on the last N rounds and passed to rounds during creation
- **Session-Level Management**: Collator priority becomes a dynamic parameter of individual rounds rather than a static session description parameter

## Optimistic Collations

Optimistic collations implement a pipeline approach to block generation, allowing collators to work ahead and reduce round latency.

### Pipeline Concept

Since the collator will most likely remain the same in the next round, it can begin collating the next block-candidate immediately after finishing the current round's collation. This creates a **pipeline of collations** using precollated block-candidates.

### Session Management

The session actively manages precollated block-candidates through a FIFO queue:

#### Block Discovery Process
1. **Check for Precollated Block**: Session searches for a precollated block-candidate for the current round
2. **If Found**: 
   - Remove from FIFO precollated queue
   - Immediately send to other validators for approval
   - Request collator to precollate the next block
3. **If Not Found**: Request collator to collate as in the original implementation

#### Queue Management
- **Collation Callback**: When collation finishes, the session checks if the precollated FIFO queue has space
- **Controlled Growth**: If space is available, the session requests the collator to generate another block-candidate using the block-candidate received from the collator as the previous block in the next generation
- **Size Limits**: Queue size is controlled to prevent resource exhaustion

### Enhanced Collator Capabilities

The collator implementation has been updated to support:
- **Context-Based Collation**: Generate blocks based on current state (original behavior)
- **Historical Collation**: Generate blocks using any previous block as a base

## Traffic Optimizations

Current implementation analysis shows a duplication factor of 3-5x for incoming catchain messages in most tests. These optimizations aim to reduce message volume and CPU processing overhead.

### All-to-All Messaging

The protocol transitions from GOSSIP to direct all-to-all message sending:

#### Benefits
1. **Reduced Latency**: Fewer hops in message delivery
2. **System Transport**: Leverages efficient system-level TCP implementation
3. **Stream-Based Communication**: More reliable than manual message delivery

#### Protocol Changes
1. **No Message Resending**: Nodes do not resend messages after receiving them (eliminates GOSSIP behavior)
2. **Rare Query Sync**: Queries are sent infrequently (once per 1-2 seconds) as a slow synchronization method for node state recovery after restarts
3. **Direct RLDP over TCP**: 
   - **Reduced Traffic**: Eliminates error correction overhead in RLDP block bodies
   - **Faster Delivery**: Uses efficient system TCP implementation
   - **Improved Reliability**: Avoids full blocks retransmission (RLDP or broadcasts) when individual packets are lost

### Performance Impact

- **Message Reduction**: Significant decrease in duplicate message processing
- **CPU Optimization**: Lower processing overhead due to reduced message volume
- **Network Efficiency**: More efficient use of available bandwidth

## Implementation Notes

### Backward Compatibility

- **Full Compatibility**: Implementation maintains backward compatibility with original node implementation
- **Configuration Control**: Accelerated consensus is enabled via new config parameter 63
- **Protocol Versioning**: New protocol version at session level prevents conflicts between updated and non-updated nodes
- **Gradual Deployment**: Allows production network upgrades before consensus activation via network config updates

### Rollout Strategy

**Phased Activation**:
1. **Shard Chains First**: Consensus optimizations will be activated for shard chains initially
2. **Master Chain Second**: Master chain activation follows successful shard deployment
3. **Separate Configuration**: Independent consensus config parameters for master chain and shard chains

This phased approach ensures stability and allows for monitoring and adjustment during the rollout process.