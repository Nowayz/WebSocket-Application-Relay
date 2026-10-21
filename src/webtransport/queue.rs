//! One consumer, serialized producers. Keep the first queued packet inline so
//! an idle peer does not retain Tokio mpsc's initial 32-message allocation.
use parking_lot::Mutex;
use std::{
    collections::VecDeque,
    future::{Future, poll_fn},
    task::Poll,
};
use tokio::sync::Notify;

pub(super) struct Queue<T> {
    pub state: Mutex<State<T>>,
    pub ready: Notify,
}

pub(super) struct State<T> {
    first: Option<T>,
    rest: VecDeque<T>,
    maximum: usize,
    closed: bool,
}

impl<T> State<T> {
    pub fn allocated_bytes(&self) -> usize {
        self.rest.capacity() * std::mem::size_of::<T>()
    }

    pub fn is_empty(&self) -> bool {
        self.first.is_none()
    }

    pub fn is_closed(&self) -> bool {
        self.closed
    }

    pub fn try_push(&mut self, value: T) -> bool {
        if self.closed || usize::from(self.first.is_some()) + self.rest.len() >= self.maximum {
            return false;
        }
        if self.first.is_none() {
            self.first = Some(value);
        } else {
            self.rest.push_back(value);
        }
        true
    }

    fn pop(&mut self) -> Option<T> {
        let value = self.first.take()?;
        self.first = self.rest.pop_front();
        Some(value)
    }
}

impl<T> Queue<T> {
    pub fn new(maximum: usize) -> Self {
        Self {
            state: Mutex::new(State {
                first: None,
                rest: VecDeque::new(),
                maximum,
                closed: false,
            }),
            ready: Notify::new(),
        }
    }

    // Only the owning session creates a receiver. Dropping it refuses future
    // submissions and releases every queued output reservation.
    pub fn receiver(&self) -> Receiver<'_, T> {
        Receiver { queue: self }
    }
}

pub(super) struct Receiver<'a, T> {
    queue: &'a Queue<T>,
}

impl<T> Receiver<'_, T> {
    pub fn close(&self) {
        self.queue.state.lock().closed = true;
        self.queue.ready.notify_one();
    }

    #[cfg(test)]
    pub async fn recv(&mut self) -> Option<T> {
        self.recv_with_idle().await.0
    }

    // Report an actual empty-queue wait so the writer can start a fresh work quantum
    // after sleeping, rather than carrying an old partial quantum into a burst.
    #[cfg(test)]
    pub async fn recv_with_idle(&mut self) -> (Option<T>, bool) {
        self.recv_with_idle_map(|value| value).await
    }

    // Transform under the queue lock when ownership must move with dequeue.
    pub async fn recv_with_idle_map<U>(
        &mut self,
        mut map: impl FnMut(T) -> U,
    ) -> (Option<U>, bool) {
        let mut was_idle = false;
        loop {
            // Most active writers already have output. Avoid registering and
            // unregistering a wakeup for each ready packet in a burst.
            {
                let mut state = self.queue.state.lock();
                if let Some(value) = state.pop() {
                    return (Some(map(value)), was_idle);
                }
                if state.closed {
                    return (None, was_idle);
                }
            }
            let notified = self.queue.ready.notified();
            tokio::pin!(notified);
            // Register before inspecting the queue to cover a concurrent enqueue.
            notified.as_mut().enable();
            {
                let mut state = self.queue.state.lock();
                if let Some(value) = state.pop() {
                    return (Some(map(value)), was_idle);
                }
                if state.closed {
                    return (None, was_idle);
                }
            }
            poll_fn(|cx| match notified.as_mut().poll(cx) {
                Poll::Pending => {
                    was_idle = true;
                    Poll::Pending
                }
                Poll::Ready(()) => Poll::Ready(()),
            })
            .await;
        }
    }
}

impl<T> Drop for Receiver<'_, T> {
    fn drop(&mut self) {
        let mut state = self.queue.state.lock();
        state.closed = true;
        state.first = None;
        state.rest.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering::Relaxed},
    };

    #[tokio::test]
    async fn dequeue_transfers_external_ownership_under_the_submission_gate() {
        let queue = Queue::new(2);
        assert!(queue.state.lock().try_push(42));
        let mut receiver = queue.receiver();
        let result = receiver
            .recv_with_idle_map(|value| {
                assert!(queue.state.try_lock().is_none());
                value + 1
            })
            .await;
        assert_eq!(result, (Some(43), false));
    }

    #[tokio::test]
    async fn idle_wait_is_distinguished_from_an_already_queued_burst() {
        let queue = Queue::new(2);
        let mut receiver = queue.receiver();
        let producer = async {
            tokio::task::yield_now().await;
            assert!(queue.state.lock().try_push(1));
            queue.ready.notify_one();
        };
        let (_, first) = tokio::join!(producer, receiver.recv_with_idle());
        assert_eq!(first, (Some(1), true));
        assert!(queue.state.lock().try_push(2));
        assert_eq!(receiver.recv_with_idle().await, (Some(2), false));
    }

    #[tokio::test]
    async fn fifo_capacity_close_and_inline_idle_storage() {
        let queue = Queue::new(3);
        let mut receive = queue.receiver();
        assert!(queue.state.lock().try_push(1));
        assert_eq!(queue.state.lock().rest.capacity(), 0);
        assert_eq!(receive.recv().await, Some(1));
        assert!(queue.state.lock().try_push(2));
        assert!(queue.state.lock().try_push(3));
        assert!(queue.state.lock().try_push(4));
        assert!(!queue.state.lock().try_push(5));
        receive.close();
        assert!(!queue.state.lock().try_push(6));
        for value in [2, 3, 4] {
            assert_eq!(receive.recv().await, Some(value));
        }
        assert_eq!(receive.recv().await, None);
    }

    #[tokio::test]
    async fn concurrent_wakeups_and_cancelled_receive_preserve_packets() {
        let queue = Arc::new(Queue::new(1024));
        let mut receive = queue.receiver();
        // Poll then cancel a waiting receive before any enqueue.
        tokio::select! {
            biased;
            _ = receive.recv() => panic!("empty queue became ready"),
            _ = tokio::task::yield_now() => {}
        }
        let producer = queue.clone();
        let task = tokio::spawn(async move {
            for value in 0..1000 {
                assert!(producer.state.lock().try_push(value));
                producer.ready.notify_one();
                tokio::task::yield_now().await;
            }
            producer.state.lock().closed = true;
            producer.ready.notify_one();
        });
        for value in 0..1000 {
            assert_eq!(receive.recv().await, Some(value));
        }
        assert_eq!(receive.recv().await, None);
        task.await.unwrap();
    }

    #[test]
    fn dropping_receiver_releases_all_items_and_refuses_late_output() {
        struct Lease(Arc<AtomicUsize>);
        impl Drop for Lease {
            fn drop(&mut self) {
                self.0.fetch_sub(1, Relaxed);
            }
        }
        let count = Arc::new(AtomicUsize::new(3));
        let queue = Queue::new(8);
        let receive = queue.receiver();
        for _ in 0..3 {
            assert!(queue.state.lock().try_push(Lease(count.clone())));
        }
        drop(receive);
        assert_eq!(count.load(Relaxed), 0);
        assert!(
            !queue
                .state
                .lock()
                .try_push(Lease(Arc::new(AtomicUsize::new(1))))
        );
    }
}
