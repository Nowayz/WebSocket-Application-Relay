//! One endpoint-owned sender and one connection-owned receiver. Keep an idle
//! event slot inline instead of allocating a 32-event block per connection.
//! The endpoint serializes producers; this channel also permits concurrent
//! send/receive. FIFO, receiver retirement and scheduler cooperation are explicit.
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    task::{ready, Context, Poll, Waker},
};

use tokio::sync::mpsc::error::SendError;

// Match the original mpsc block retention for small bursts. This is a storage
// reuse threshold, never a limit on queued or delivered events.
const RETAIN_SLOTS: usize = if cfg!(target_pointer_width = "64") {
    32
} else {
    16
};

#[derive(Debug)]
struct State<T> {
    first: Option<T>,
    rest: VecDeque<T>,
    sender_alive: bool,
    receiver_alive: bool,
    reader: Option<Waker>,
}

#[derive(Debug)]
pub(crate) struct Sender<T>(Arc<Mutex<State<T>>>);

#[derive(Debug)]
pub(crate) struct Receiver<T>(Arc<Mutex<State<T>>>);

pub(crate) fn channel<T>() -> (Sender<T>, Receiver<T>) {
    let state = Arc::new(Mutex::new(State {
        first: None,
        rest: VecDeque::new(),
        sender_alive: true,
        receiver_alive: true,
        reader: None,
    }));
    (Sender(state.clone()), Receiver(state))
}

impl<T> Sender<T> {
    pub(crate) fn send(&self, value: T) -> Result<(), SendError<T>> {
        let wake = {
            let mut state = self.0.lock().unwrap();
            if !state.receiver_alive {
                return Err(SendError(value));
            }
            if state.first.is_none() && state.rest.is_empty() {
                state.first = Some(value);
            } else {
                state.rest.push_back(value);
            }
            state.reader.take()
        };
        if let Some(waker) = wake {
            waker.wake();
        }
        Ok(())
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        let wake = {
            let mut state = self
                .0
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.sender_alive = false;
            state.reader.take()
        };
        if let Some(waker) = wake {
            waker.wake();
        }
    }
}

impl<T> Receiver<T> {
    pub(crate) fn poll_recv(&mut self, cx: &mut Context<'_>) -> Poll<Option<T>> {
        // Match Tokio mpsc's budget consumption. A permanently ready packet
        // source must still let connection writes and other tasks run.
        let coop = ready!(tokio::task::coop::poll_proceed(cx));
        let mut state = self.0.lock().unwrap();
        let value = state.first.take().or_else(|| state.rest.pop_front());
        if state.rest.is_empty() && state.rest.capacity() > RETAIN_SLOTS {
            state.rest = VecDeque::new();
        }
        if value.is_some() || !state.sender_alive {
            coop.made_progress();
            return Poll::Ready(value);
        }
        if let Some(reader) = &mut state.reader {
            reader.clone_from(cx.waker());
        } else {
            state.reader = Some(cx.waker().clone());
        }
        Poll::Pending
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        let retired = {
            let mut state = self
                .0
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.receiver_alive = false;
            (
                state.first.take(),
                std::mem::take(&mut state.rest),
                state.reader.take(),
            )
        };
        // Releasing events or a retired task's waker can run destructors.
        drop(retired);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        cell::Cell,
        future::poll_fn,
        sync::atomic::{AtomicUsize, Ordering::Relaxed},
        task::Wake,
    };

    #[tokio::test]
    async fn fifo_burst_drain_and_inline_idle_storage() {
        let (send, mut receive) = channel();
        send.send(0).unwrap();
        assert_eq!(send.0.lock().unwrap().rest.capacity(), 0);
        for value in 1..1000 {
            send.send(value).unwrap();
        }
        drop(send);
        for value in 0..1000 {
            assert_eq!(poll_fn(|cx| receive.poll_recv(cx)).await, Some(value));
        }
        assert_eq!(poll_fn(|cx| receive.poll_recv(cx)).await, None);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn concurrent_producer_and_cancelled_receive_preserve_fifo() {
        let (send, mut receive) = channel();
        tokio::select! {
            biased;
            _ = poll_fn(|cx| receive.poll_recv(cx)) => panic!("empty queue became ready"),
            _ = tokio::task::yield_now() => {}
        }
        let producer = tokio::spawn(async move {
            for value in 0..1000 {
                send.send(value).unwrap();
                tokio::task::yield_now().await;
            }
        });
        for value in 0..1000 {
            assert_eq!(poll_fn(|cx| receive.poll_recv(cx)).await, Some(value));
        }
        producer.await.unwrap();
        assert_eq!(poll_fn(|cx| receive.poll_recv(cx)).await, None);
    }

    #[test]
    fn sender_retirement_wakes_reader_and_receiver_retirement_releases_events() {
        struct Counter(AtomicUsize);
        impl Wake for Counter {
            fn wake(self: Arc<Self>) {
                self.0.fetch_add(1, Relaxed);
            }
        }
        let counter = Arc::new(Counter(AtomicUsize::new(0)));
        let waker = Waker::from(counter.clone());
        let mut cx = Context::from_waker(&waker);
        let (send, mut receive) = channel::<usize>();
        assert!(receive.poll_recv(&mut cx).is_pending());
        drop(send);
        assert_eq!(counter.0.load(Relaxed), 1);
        assert_eq!(receive.poll_recv(&mut cx), Poll::Ready(None));

        let value = Arc::new(());
        let (send, receive) = channel();
        send.send(value.clone()).unwrap();
        send.send(value.clone()).unwrap();
        drop(receive);
        assert_eq!(Arc::strong_count(&value), 1);
        let refused = send.send(value.clone()).unwrap_err();
        assert!(Arc::ptr_eq(&refused.0, &value));
        drop(refused);
        assert_eq!(Arc::strong_count(&value), 1);
    }

    #[tokio::test]
    async fn a_large_burst_releases_excess_storage_after_drain() {
        let (send, mut receive) = channel();
        for value in 0..10000 {
            send.send(value).unwrap();
        }
        assert!(send.0.lock().unwrap().rest.capacity() > 32);
        for value in 0..10000 {
            assert_eq!(poll_fn(|cx| receive.poll_recv(cx)).await, Some(value));
        }
        let capacity = send.0.lock().unwrap().rest.capacity();
        assert_eq!(capacity, 0);
        send.send(10000).unwrap();
        assert_eq!(poll_fn(|cx| receive.poll_recv(cx)).await, Some(10000));
    }

    #[test]
    fn retirement_after_an_unwind_releases_events_without_a_second_panic() {
        let value = Arc::new(());
        let (send, receive) = channel();
        send.send(value.clone()).unwrap();
        let result = std::panic::catch_unwind(|| {
            let _state = send.0.lock().unwrap();
            panic!("simulate a driver unwinding while the queue is locked");
        });
        assert!(result.is_err());
        drop(receive);
        drop(send);
        assert_eq!(Arc::strong_count(&value), 1);
    }

    #[tokio::test]
    async fn a_ready_queue_preserves_tokio_cooperative_scheduling() {
        let (send, mut receive) = channel();
        for value in 0..1000 {
            send.send(value).unwrap();
        }
        drop(send);
        let count = Cell::new(0);
        tokio::join!(biased;
            async { while poll_fn(|cx| receive.poll_recv(cx)).await.is_some() { count.set(count.get() + 1); } },
            async { assert!(count.get() < 1000, "ready input starved another future"); }
        );
        assert_eq!(count.get(), 1000);
    }
}
