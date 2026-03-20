# ADNL

ADNL (Abstract Datagram Network Layer) protocol stack implementation over UDP and TCP.

## Protocols

- **ADNL** — low-level encrypted datagram protocol for all TON node communication
- **RLDP** — Reliable Large Datagram Protocol on top of ADNL UDP, uses FEC for large data transfers
- **Overlay** — network partitioning into public and private subnetworks (overlays)
- **DHT** — distributed hash table for node discovery and metadata storage
