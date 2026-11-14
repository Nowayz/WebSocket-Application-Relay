use super::*;
use std::{
    future::Future,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering::Relaxed},
    },
    task::Wake,
};

#[tokio::test]
async fn dequeue_updates_owner_under_the_submission_gate() {
    let queue = Queue::with_owner(2, false);
    assert!(queue.state.lock().try_push(42));
    let mut receiver = queue.receiver();
    let result = receiver
        .recv_with_idle_map(|value, busy| {
            assert!(queue.state.try_lock().is_none());
            assert!(!*busy);
            *busy = true;
            value + 1
        })
        .await;
    assert_eq!(result, (Some(43), false));
    assert!(queue.state.lock().owner);
}

#[tokio::test]
async fn idle_wait_is_distinguished_from_an_already_queued_burst() {
    let queue = Queue::new(2);
    let mut receiver = queue.receiver();
    let producer = async {
        tokio::task::yield_now().await;
        let mut state = queue.state.lock();
        assert!(state.try_push(1));
        state.wake_consumer();
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
            {
                let mut state = producer.state.lock();
                assert!(state.try_push(value));
                state.wake_consumer();
            }
            tokio::task::yield_now().await;
        }
        let mut state = producer.state.lock();
        state.closed = true;
        state.wake_consumer();
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
    let mut receive = queue.receiver();
    let counter = Arc::new(WakeCounter::default());
    let weak = Arc::downgrade(&counter);
    {
        let waker = Waker::from(counter);
        let mut cx = Context::from_waker(&waker);
        assert!(
            receive
                .poll_recv_with_idle_map(&mut cx, |value, _| value)
                .is_pending()
        );
    }
    assert!(weak.upgrade().is_some());
    for _ in 0..3 {
        assert!(queue.state.lock().try_push(Lease(count.clone())));
    }
    drop(receive);
    assert_eq!(count.load(Relaxed), 0);
    assert!(weak.upgrade().is_none());
    assert!(queue.state.lock().consumer.is_none());
    assert!(queue.state.lock().is_closed());
    assert!(
        !queue
            .state
            .lock()
            .try_push(Lease(Arc::new(AtomicUsize::new(1))))
    );
}

#[derive(Default)]
struct WakeCounter(AtomicUsize);

impl Wake for WakeCounter {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.fetch_add(1, Relaxed);
    }
}

#[test]
fn enqueue_before_and_after_registration_never_loses_output_or_idle_state() {
    let queue = Queue::new(3);
    let mut receive = queue.receiver();
    let counter = Arc::new(WakeCounter::default());
    let waker = Waker::from(counter.clone());
    let mut cx = Context::from_waker(&waker);
    {
        let mut state = queue.state.lock();
        assert!(state.try_push(10));
        state.wake_consumer();
    }
    assert_eq!(counter.0.load(Relaxed), 0);
    assert_eq!(
        receive.poll_recv_with_idle_map(&mut cx, |value, _| value),
        Poll::Ready((Some(10), false))
    );
    assert!(
        receive
            .poll_recv_with_idle_map(&mut cx, |value, _| value)
            .is_pending()
    );
    {
        let mut state = queue.state.lock();
        assert!(state.try_push(20));
        state.wake_consumer();
        state.wake_consumer();
    }
    assert_eq!(counter.0.load(Relaxed), 1);
    assert_eq!(
        receive.poll_recv_with_idle_map(&mut cx, |value, _| value),
        Poll::Ready((Some(20), true))
    );
    assert!(queue.state.lock().try_push(30));
    assert_eq!(
        receive.poll_recv_with_idle_map(&mut cx, |value, _| value),
        Poll::Ready((Some(30), false))
    );
}

#[test]
fn a_pending_receiver_uses_the_latest_task_waker() {
    let queue = Queue::<usize>::new(1);
    let mut receive = queue.receiver();
    let old = Arc::new(WakeCounter::default());
    let new = Arc::new(WakeCounter::default());
    let old_waker = Waker::from(old.clone());
    let new_waker = Waker::from(new.clone());
    assert!(
        receive
            .poll_recv_with_idle_map(&mut Context::from_waker(&old_waker), |value, _| value)
            .is_pending()
    );
    assert!(
        receive
            .poll_recv_with_idle_map(&mut Context::from_waker(&new_waker), |value, _| value)
            .is_pending()
    );
    {
        let mut state = queue.state.lock();
        assert!(state.try_push(1));
        state.wake_consumer();
    }
    assert_eq!(old.0.load(Relaxed), 0);
    assert_eq!(new.0.load(Relaxed), 1);
    assert_eq!(
        receive.poll_recv_with_idle_map(&mut Context::from_waker(&new_waker), |value, _| value),
        Poll::Ready((Some(1), true))
    );
}

#[test]
fn closing_a_pending_receiver_wakes_it_and_refuses_late_output() {
    let queue = Queue::<usize>::new(1);
    let mut receive = queue.receiver();
    let counter = Arc::new(WakeCounter::default());
    let waker = Waker::from(counter.clone());
    let mut cx = Context::from_waker(&waker);
    assert!(
        receive
            .poll_recv_with_idle_map(&mut cx, |value, _| value)
            .is_pending()
    );
    receive.close();
    assert_eq!(counter.0.load(Relaxed), 1);
    assert_eq!(
        receive.poll_recv_with_idle_map(&mut cx, |value, _| value),
        Poll::Ready((None, true))
    );
    assert_eq!(
        receive.poll_recv_with_idle_map(&mut cx, |value, _| value),
        Poll::Ready((None, false))
    );
    assert!(!queue.state.lock().try_push(1));
}

#[test]
fn cancelling_the_async_adapter_unregisters_without_losing_later_output() {
    let queue = Queue::new(1);
    let mut receive = queue.receiver();
    let counter = Arc::new(WakeCounter::default());
    let waker = Waker::from(counter);
    let mut cx = Context::from_waker(&waker);
    {
        let waiting = receive.recv_with_idle();
        tokio::pin!(waiting);
        assert!(waiting.as_mut().poll(&mut cx).is_pending());
    }
    assert!(queue.state.lock().consumer.is_none());
    assert!(queue.state.lock().try_push(7));
    let waiting = receive.recv_with_idle();
    tokio::pin!(waiting);
    assert_eq!(
        waiting.as_mut().poll(&mut cx),
        Poll::Ready((Some(7), false))
    );
}
