//! The click pipeline: the redirect emits, a background task batches and writes.
//!
//! The redirect must never wait on ClickHouse. So the hot path only pushes onto
//! a bounded channel — and if the channel is full it **drops** the event and
//! counts the drop, rather than blocking. Analytics is best-effort by
//! construction: a lost click is a rounding error, a slowed redirect is the
//! product getting worse.

use tokio::sync::mpsc;

use super::{ClickEvent, ClickSink};

/// How many events the channel holds before the redirect starts dropping. Sized
/// to absorb a burst while the writer works through a batch, not to be a durable
/// queue — durability is not a property analytics needs.
const CHANNEL_CAPACITY: usize = 8192;

/// How many events accumulate before a write, and how long to wait before
/// flushing a partial batch. ClickHouse wants large inserts; the interval keeps
/// a trickle of clicks from sitting unwritten forever.
const BATCH_SIZE: usize = 512;
const FLUSH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

/// The emit side, held in `AppState`. Cloneable and cheap.
///
/// `None` when analytics is disabled, so [`Self::emit`] is a branch and nothing
/// more — the redirect pays no channel cost when there is no sink to feed.
#[derive(Debug, Clone)]
pub struct ClickTx {
    tx: Option<mpsc::Sender<ClickEvent>>,
}

impl ClickTx {
    /// A sender that goes nowhere, for a disabled sink.
    pub const fn disabled() -> Self {
        Self { tx: None }
    }

    /// Records a click, or gives up instantly if the queue is full.
    ///
    /// `try_send`, never `send().await`: the redirect returns now, and a full
    /// queue means the writer is behind, which is precisely when the redirect
    /// must not wait. The drop is counted so a saturated pipeline is visible in
    /// metrics rather than silent.
    pub fn emit(&self, event: ClickEvent) {
        let Some(tx) = &self.tx else {
            return;
        };

        if tx.try_send(event).is_err() {
            metrics::counter!("click_events_dropped_total").increment(1);
        }
    }
}

/// Spawns the writer and returns the sender the redirect emits through.
///
/// When the sink is disabled there is no writer and no channel — the sender is a
/// no-op. Otherwise a task owns the sink and drains the channel in batches until
/// the process ends.
pub fn spawn(sink: ClickSink) -> ClickTx {
    if !sink.is_active() {
        return ClickTx::disabled();
    }

    let (tx, rx) = mpsc::channel(CHANNEL_CAPACITY);
    tokio::spawn(run(sink, rx));
    ClickTx { tx: Some(tx) }
}

/// Drains the channel, writing on a full batch or a timer, whichever comes first.
///
/// A failed write drops the batch and moves on: retrying would grow memory under
/// exactly the condition — ClickHouse struggling — where that is most dangerous,
/// and the events are best-effort anyway. The failure is logged.
async fn run(sink: ClickSink, mut rx: mpsc::Receiver<ClickEvent>) {
    let mut batch: Vec<ClickEvent> = Vec::with_capacity(BATCH_SIZE);
    let mut ticker = tokio::time::interval(FLUSH_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            received = rx.recv() => {
                // `None` means every sender is gone — the app is shutting down.
                // Write what is buffered and stop.
                let Some(event) = received else {
                    flush(&sink, &mut batch).await;
                    break;
                };

                batch.push(event);
                if batch.len() >= BATCH_SIZE {
                    flush(&sink, &mut batch).await;
                }
            }
            _ = ticker.tick() => {
                flush(&sink, &mut batch).await;
            }
        }
    }
}

async fn flush(sink: &ClickSink, batch: &mut Vec<ClickEvent>) {
    if batch.is_empty() {
        return;
    }

    if let Err(err) = sink.record(batch).await {
        tracing::warn!(%err, count = batch.len(), "dropping a batch of click events");
    }

    batch.clear();
}
