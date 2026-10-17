//! A native worker owns these cumulative deltas until the end of its ready
//! batch. Admission and retained-byte gauges remain immediate shared atomics.
use std::{cell::Cell, marker::PhantomData, rc::Rc, sync::atomic::Ordering::Relaxed};

use super::Metrics;

#[derive(Default)]
struct Local {
    owner: Cell<usize>,
    incoming_messages: Cell<u64>,
    incoming_bytes: Cell<u64>,
    reused_bytes: Cell<u64>,
    copied_bytes: Cell<u64>,
    deliveries: Cell<u64>,
    delivered_bytes: Cell<u64>,
}

thread_local! {
    static TRAFFIC: Local = const { Local {
        owner: Cell::new(0),
        incoming_messages: Cell::new(0), incoming_bytes: Cell::new(0),
        reused_bytes: Cell::new(0), copied_bytes: Cell::new(0),
        deliveries: Cell::new(0), delivered_bytes: Cell::new(0),
    } };
}

fn add(cell: &Cell<u64>, value: u64) {
    cell.set(cell.get().wrapping_add(value));
}

/// The borrow keeps the metrics identity alive and immovable. The marker keeps
/// publication and scope retirement on the thread that owns the deltas.
pub(crate) struct TrafficBatch<'a> {
    metrics: &'a Metrics,
    active: bool,
    _thread: PhantomData<Rc<()>>,
}

impl Metrics {
    pub(crate) fn batch_traffic(&self) -> TrafficBatch<'_> {
        let active = TRAFFIC.with(|local| {
            if local.owner.get() != 0 {
                return false;
            }
            local.owner.set(self as *const Self as usize);
            true
        });
        TrafficBatch {
            metrics: self,
            active,
            _thread: PhantomData,
        }
    }

    #[inline]
    fn local_traffic(&self, record: impl FnOnce(&Local)) -> bool {
        TRAFFIC.with(|local| {
            if local.owner.get() != self as *const Self as usize {
                return false;
            }
            record(local);
            true
        })
    }

    #[inline]
    pub(crate) fn record_incoming(&self, bytes: usize) {
        if !self.local_traffic(|local| {
            add(&local.incoming_messages, 1);
            add(&local.incoming_bytes, bytes as u64);
        }) {
            self.incoming_messages.fetch_add(1, Relaxed);
            self.incoming_bytes.fetch_add(bytes as u64, Relaxed);
        }
    }

    #[inline]
    pub(crate) fn record_routed(&self, bytes: usize, copied: bool) {
        if !self.local_traffic(|local| {
            add(
                if copied {
                    &local.copied_bytes
                } else {
                    &local.reused_bytes
                },
                bytes as u64,
            );
        }) {
            let counter = if copied {
                &self.routing_copied_bytes
            } else {
                &self.routing_reused_bytes
            };
            counter.fetch_add(bytes as u64, Relaxed);
        }
    }

    #[inline]
    pub(crate) fn record_delivery(&self, bytes: usize) {
        if !self.local_traffic(|local| {
            add(&local.deliveries, 1);
            add(&local.delivered_bytes, bytes as u64);
        }) {
            self.deliveries.fetch_add(1, Relaxed);
            self.delivered_bytes.fetch_add(bytes as u64, Relaxed);
        }
    }
}

impl TrafficBatch<'_> {
    /// Publish before sleeping, even if there are no more socket events. Only
    /// totals change here: resource reservations never depend on this batch.
    pub(crate) fn publish(&self) {
        if !self.active {
            return;
        }
        TRAFFIC.with(|local| {
            for (global, delta) in [
                (&self.metrics.incoming_messages, &local.incoming_messages),
                (&self.metrics.incoming_bytes, &local.incoming_bytes),
                (&self.metrics.routing_reused_bytes, &local.reused_bytes),
                (&self.metrics.routing_copied_bytes, &local.copied_bytes),
                (&self.metrics.deliveries, &local.deliveries),
                (&self.metrics.delivered_bytes, &local.delivered_bytes),
            ] {
                let value = delta.replace(0);
                if value != 0 {
                    global.fetch_add(value, Relaxed);
                }
            }
        });
    }
}

impl Drop for TrafficBatch<'_> {
    fn drop(&mut self) {
        if self.active {
            self.publish();
            TRAFFIC.with(|local| local.owner.set(0));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batching_is_scoped_to_one_metrics_instance_and_publishes_on_exit() {
        let metrics = Metrics::default();
        let other = Metrics::default();
        {
            let batch = metrics.batch_traffic();
            metrics.record_incoming(30);
            metrics.record_routed(24, false);
            metrics.record_delivery(28);
            assert_eq!(metrics.incoming_messages.load(Relaxed), 0);
            metrics.queued_bytes.fetch_add(28, Relaxed);
            assert_eq!(metrics.queued_bytes.load(Relaxed), 28);
            {
                let _nested = other.batch_traffic();
                other.record_incoming(7);
                assert_eq!(other.incoming_bytes.load(Relaxed), 7);
            }
            batch.publish();
            batch.publish();
            assert_eq!(metrics.incoming_messages.load(Relaxed), 1);
            assert_eq!(metrics.incoming_bytes.load(Relaxed), 30);
            assert_eq!(metrics.deliveries.load(Relaxed), 1);
            assert_eq!(metrics.delivered_bytes.load(Relaxed), 28);
            assert_eq!(metrics.routing_reused_bytes.load(Relaxed), 24);
            metrics.record_routed(8, true);
        }
        assert_eq!(metrics.routing_copied_bytes.load(Relaxed), 8);
        metrics.record_delivery(10);
        assert_eq!(metrics.deliveries.load(Relaxed), 2);
        assert_eq!(metrics.delivered_bytes.load(Relaxed), 38);
    }

    #[test]
    fn worker_exit_and_unwinding_preserve_all_deltas() {
        let metrics = std::sync::Arc::new(Metrics::default());
        let threads: Vec<_> = (0..4)
            .map(|worker| {
                let metrics = metrics.clone();
                std::thread::spawn(move || {
                    let _batch = metrics.batch_traffic();
                    for _ in 0..1000 {
                        metrics.record_incoming(30);
                        metrics.record_delivery(28);
                    }
                    if worker == 3 {
                        panic!("exercise batch retirement");
                    }
                })
            })
            .collect();
        for (index, thread) in threads.into_iter().enumerate() {
            assert_eq!(thread.join().is_err(), index == 3);
        }
        assert_eq!(metrics.incoming_messages.load(Relaxed), 4000);
        assert_eq!(metrics.incoming_bytes.load(Relaxed), 120000);
        assert_eq!(metrics.deliveries.load(Relaxed), 4000);
        assert_eq!(metrics.delivered_bytes.load(Relaxed), 112000);
    }
}
