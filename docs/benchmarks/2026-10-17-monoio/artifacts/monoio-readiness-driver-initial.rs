//! Experimental Monoio io_uring readiness driver. The existing WebSocket
//! decoder, worker scratch storage and borrowed vectored socket output remain
//! unchanged. This deliberately tests readiness, not owned-buffer completion I/O.
use std::{future::poll_fn, rc::Rc, task::Poll as TaskPoll};

use futures_util::{
    FutureExt, StreamExt,
    future::{AbortHandle, Abortable, LocalBoxFuture},
    stream::FuturesUnordered,
};

use super::*;

pub(super) fn run(worker: Worker) -> Result<()> {
    // Require io_uring explicitly: an unavailable ring must fail startup rather
    // than silently measuring epoll under a different name.
    monoio::RuntimeBuilder::<monoio::IoUringDriver>::new()
        .enable_timer()
        .build()
        .context("cannot create Monoio io_uring worker")?
        .block_on(run_worker(worker))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Direction {
    Read,
    Write,
}

type Completion = Option<(Token, Direction, u64, io::Result<()>)>;

struct Watch {
    stream: Rc<monoio::net::TcpStream>,
    read: Option<AbortHandle>,
    write: Option<AbortHandle>,
    read_generation: u64,
    write_generation: u64,
}

impl Drop for Watch {
    fn drop(&mut self) {
        if let Some(handle) = self.read.take() {
            handle.abort();
        }
        if let Some(handle) = self.write.take() {
            handle.abort();
        }
    }
}

struct Driver {
    listener: Rc<monoio::net::TcpListener>,
    listener_wait: Option<AbortHandle>,
    listener_generation: u64,
    watches: HashMap<Token, Watch>,
    waits: FuturesUnordered<LocalBoxFuture<'static, Completion>>,
}

impl Driver {
    fn new(listener: std::net::TcpListener) -> io::Result<Self> {
        Ok(Self {
            listener: Rc::new(monoio::net::TcpListener::from_std(listener)?),
            listener_wait: None,
            listener_generation: 0,
            watches: HashMap::new(),
            waits: FuturesUnordered::new(),
        })
    }

    fn sync(&mut self, worker: &mut Worker, stopping: bool) -> Result<()> {
        if !stopping && worker.accept_retry.is_none() {
            if self.listener_wait.is_none() {
                self.listener_generation += 1;
                let generation = self.listener_generation;
                let listener = self.listener.clone();
                let (abort, registration) = AbortHandle::new_pair();
                self.waits.push(
                    Abortable::new(
                        async move {
                            (
                                LISTENER,
                                Direction::Read,
                                generation,
                                listener.readable(false).await,
                            )
                        },
                        registration,
                    )
                    .map(Result::ok)
                    .boxed_local(),
                );
                self.listener_wait = Some(abort);
            }
        } else if let Some(wait) = self.listener_wait.take() {
            self.listener_generation += 1;
            wait.abort();
        }

        for (token, stream) in worker.monoio_new.drain(..) {
            // A newly accepted connection can already have failed its first
            // synchronous handshake turn. Do not install a stale watcher.
            if worker.tokens.contains_key(&token) {
                self.watches.insert(
                    token,
                    Watch {
                        stream: Rc::new(monoio::net::TcpStream::from_std(stream)?),
                        read: None,
                        write: None,
                        read_generation: 0,
                        write_generation: 0,
                    },
                );
            }
        }
        for token in worker.monoio_dirty.drain(..) {
            let Some(&slot) = worker.tokens.get(&token) else {
                self.watches.remove(&token);
                continue;
            };
            let Some(watch) = self.watches.get_mut(&token) else {
                continue;
            };
            if watch.read.is_none() {
                watch.read_generation += 1;
                watch.read = Some(Self::arm(
                    &self.waits,
                    watch.stream.clone(),
                    token,
                    Direction::Read,
                    watch.read_generation,
                ));
            }
            if worker.connections[slot].socket.state.lock().writable {
                if watch.write.is_none() {
                    watch.write_generation += 1;
                    watch.write = Some(Self::arm(
                        &self.waits,
                        watch.stream.clone(),
                        token,
                        Direction::Write,
                        watch.write_generation,
                    ));
                }
            } else if let Some(wait) = watch.write.take() {
                watch.write_generation += 1;
                wait.abort();
            }
        }
        if self.watches.capacity() > 256 && self.watches.len() <= self.watches.capacity() / 8 {
            self.watches.shrink_to(self.watches.len() * 2);
        }
        Ok(())
    }

    fn arm(
        waits: &FuturesUnordered<LocalBoxFuture<'static, Completion>>,
        stream: Rc<monoio::net::TcpStream>,
        token: Token,
        direction: Direction,
        generation: u64,
    ) -> AbortHandle {
        let (abort, registration) = AbortHandle::new_pair();
        waits.push(
            Abortable::new(
                async move {
                    let result = match direction {
                        Direction::Read => stream.readable(false).await,
                        Direction::Write => stream.writable(false).await,
                    };
                    (token, direction, generation, result)
                },
                registration,
            )
            .map(Result::ok)
            .boxed_local(),
        );
        abort
    }

    fn complete(&mut self, worker: &mut Worker, result: Completion, stopping: bool) -> Result<()> {
        let Some((token, direction, generation, result)) = result else {
            return Ok(());
        };
        if token == LISTENER {
            if generation != self.listener_generation {
                return Ok(());
            }
            self.listener_wait.take();
            result.context("Monoio listener readiness failed")?;
            if !stopping && worker.accept_retry.is_none() {
                worker.schedule_accept();
            }
        } else if let Some(watch) = self.watches.get_mut(&token) {
            let (current, handle) = match direction {
                Direction::Read => (watch.read_generation, &mut watch.read),
                Direction::Write => (watch.write_generation, &mut watch.write),
            };
            if generation != current {
                return Ok(());
            }
            handle.take();
            if result.is_err()
                && let Some(&slot) = worker.tokens.get(&token)
            {
                worker.connections[slot].socket.state.lock().fatal = true;
            }
            worker.schedule(token);
        }
        Ok(())
    }
}

async fn yield_once() {
    let mut yielded = false;
    poll_fn(|cx| {
        if yielded {
            TaskPoll::Ready(())
        } else {
            yielded = true;
            cx.waker().wake_by_ref();
            TaskPoll::Pending
        }
    })
    .await
}

async fn run_worker(mut worker: Worker) -> Result<()> {
    let mut driver = Driver::new(worker.monoio_listener.take().expect("Monoio listener"))?;
    let relay = worker.relay.clone();
    let mut incoming = relay.receive_batch();
    let mut shutdown_deadline = None;
    loop {
        if worker.signal.stopping.load(Acquire) && shutdown_deadline.is_none() {
            shutdown_deadline =
                Some(Instant::now() + Duration::from_secs(relay.config.shutdown_timeout_secs));
            for (_, connection) in worker.connections.iter_mut() {
                if !connection.scheduled {
                    connection.scheduled = true;
                    worker.ready.push_back(connection.token);
                }
            }
        }
        let stopping = shutdown_deadline.is_some();
        if shutdown_deadline.is_some_and(|d| d <= Instant::now())
            || (stopping && worker.connections.is_empty())
        {
            break;
        }
        let now = Instant::now();
        if !stopping && worker.accept_retry.is_some_and(|d| d <= now) {
            worker.accept_retry = None;
            worker.accept()?;
        }
        while let Some(slot) = worker.timers.pop_due(now) {
            if let Some(connection) = worker.connections.get(slot) {
                worker.schedule(connection.token);
            }
        }
        worker.drive_ready(stopping, &mut incoming)?;
        incoming.publish();
        {
            let mut pending = worker.signal.pending.lock();
            while let Some(token) = pending.pop_front() {
                let Some(&slot) = worker.tokens.get(&token) else {
                    continue;
                };
                if let Some(connection) = worker.connections.get_mut(slot)
                    && connection.token == token
                    && !connection.scheduled
                {
                    connection.scheduled = true;
                    worker.ready.push_back(token);
                }
            }
        }
        driver.sync(&mut worker, stopping)?;
        if stopping && worker.connections.is_empty() {
            break;
        }
        // Consume a bounded completion batch before an existing ready
        // continuation. No full-population scans occur in the active path.
        for _ in 0..1024 {
            let Some(Some(result)) = driver.waits.next().now_or_never() else {
                break;
            };
            driver.complete(&mut worker, result, stopping)?;
        }
        if !worker.ready.is_empty() {
            yield_once().await;
            continue;
        }
        let mut deadline = worker.timers.next();
        for next in [shutdown_deadline, worker.accept_retry]
            .into_iter()
            .flatten()
        {
            deadline = Some(deadline.map_or(next, |previous| previous.min(next)));
        }
        let timer = async {
            if let Some(deadline) = deadline {
                monoio::time::sleep(deadline.saturating_duration_since(Instant::now())).await;
            } else {
                std::future::pending::<()>().await;
            }
        };
        let signal = worker.signal.clone();
        let notified = poll_fn(|cx| {
            signal
                .monoio
                .as_ref()
                .expect("Monoio waker")
                .register(cx.waker());
            if (!stopping && signal.stopping.load(Acquire)) || !signal.pending.lock().is_empty() {
                TaskPoll::Ready(())
            } else {
                TaskPoll::Pending
            }
        });
        tokio::select! {
            result = driver.waits.next(), if !driver.waits.is_empty() => {
                if let Some(result) = result { driver.complete(&mut worker, result, stopping)?; }
            }
            _ = notified => {}
            _ = timer => {}
        }
    }
    // Cancel outstanding readiness operations. Monoio owns their FD references
    // until CQE retirement; runtime destruction drains that cancellation work.
    driver.watches.clear();
    if let Some(wait) = driver.listener_wait.take() {
        wait.abort();
    }
    while driver.waits.next().await.is_some() {}
    Ok(())
}
