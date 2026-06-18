use std::sync::Arc;
use ton_block::BlockIdExt;

const CONFIRMED_BLOCK_CHANNEL_CAPACITY: usize = 128;
const CONFIRMED_BLOCK_INPUT_CAPACITY: usize = 128;
const CONFIRMED_BLOCK_DEDUP_CAPACITY: usize = 4096;

bitflags::bitflags! {
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub struct ConfirmedBlockSource: u8 {
        const PRE_APPLIED = 0b0000_0001;
        const APPLIED = 0b0000_0010;
    }
}

#[derive(Clone)]
pub struct ConfirmedBlockEvents {
    inner: Arc<ConfirmedBlockEventsInner>,
}

#[derive(Clone)]
pub struct ConfirmedBlockEvent {
    pub id: BlockIdExt,
    pub data: Arc<Vec<u8>>,
    pub source: ConfirmedBlockSource,
}

struct ConfirmedBlockEventsInner {
    input_tx: tokio::sync::mpsc::Sender<ConfirmedBlockEvent>,
    broadcast_tx: tokio::sync::broadcast::Sender<ConfirmedBlockEvent>,
}

impl ConfirmedBlockEvents {
    /// Creates confirmed block event pipeline and spawns its dedup worker.
    ///
    /// Must be called from an active Tokio runtime.
    pub fn new() -> Self {
        let (input_tx, input_rx) = tokio::sync::mpsc::channel(CONFIRMED_BLOCK_INPUT_CAPACITY);
        let (broadcast_tx, _) = tokio::sync::broadcast::channel(CONFIRMED_BLOCK_CHANNEL_CAPACITY);
        tokio::spawn(run_confirmed_block_events(input_rx, broadcast_tx.clone()));
        Self { inner: Arc::new(ConfirmedBlockEventsInner { input_tx, broadcast_tx }) }
    }

    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<ConfirmedBlockEvent> {
        self.inner.broadcast_tx.subscribe()
    }

    pub fn notify(&self, event: ConfirmedBlockEvent) {
        match self.inner.input_tx.try_send(event) {
            Ok(()) => {}
            Err(tokio::sync::mpsc::error::TrySendError::Full(event)) => {
                log::warn!("confirmed block event queue is full; dropping {}", event.id);
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(event)) => {
                log::warn!("confirmed block event queue is closed; dropping {}", event.id);
            }
        }
    }
}

async fn run_confirmed_block_events(
    mut input_rx: tokio::sync::mpsc::Receiver<ConfirmedBlockEvent>,
    broadcast_tx: tokio::sync::broadcast::Sender<ConfirmedBlockEvent>,
) {
    let capacity = std::num::NonZeroUsize::new(CONFIRMED_BLOCK_DEDUP_CAPACITY)
        .expect("confirmed block dedup capacity must be non-zero");
    // Bounded dedup cache. A shard block can be observed as PRE_APPLIED and
    // later as APPLIED; SSE emits it only once while the worker keeps the
    // merged source mask without growing memory unbounded.
    let mut seen = lru::LruCache::new(capacity);

    while let Some(event) = input_rx.recv().await {
        let previous = seen.get(&event.id).copied().unwrap_or_else(ConfirmedBlockSource::empty);
        let first_seen = previous.is_empty();
        seen.put(event.id.clone(), previous | event.source);

        if first_seen {
            let _ = broadcast_tx.send(event);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ton_block::{ShardIdent, UInt256};

    #[tokio::test]
    async fn confirmed_block_events_deduplicate_block_ids() {
        let events = ConfirmedBlockEvents::new();
        let mut rx = events.subscribe();
        let block_id = BlockIdExt::with_params(
            ShardIdent::with_tagged_prefix(0, 0x8000_0000_0000_0000).unwrap(),
            1,
            UInt256::from([1; 32]),
            UInt256::from([2; 32]),
        );
        let data = Arc::new(Vec::new());

        let event1 = ConfirmedBlockEvent {
            id: block_id.clone(),
            data: data.clone(),
            source: ConfirmedBlockSource::PRE_APPLIED,
        };
        let event2 = ConfirmedBlockEvent {
            id: block_id.clone(),
            data,
            source: ConfirmedBlockSource::APPLIED,
        };

        events.notify(event1);
        events.notify(event2);

        let received = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
            .await
            .expect("confirmed block event timed out")
            .expect("confirmed block event channel closed");
        assert_eq!(received.id, block_id);
        assert!(tokio::time::timeout(std::time::Duration::from_millis(50), rx.recv())
            .await
            .is_err());
    }
}
