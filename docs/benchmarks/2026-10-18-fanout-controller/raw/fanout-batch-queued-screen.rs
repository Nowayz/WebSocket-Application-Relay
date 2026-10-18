
#[cfg(test)]
mod queued_ack_screen {
    use super::*;
async fn acknowledge_old(
    acks: &mut mpsc::Receiver<usize>,
    jobs: &mut JoinSet<ReaderResult>,
    seen: &mut [bool],
) -> Result<()> {
    seen.fill(false);
    for _ in 0..seen.len() {
        let index = timeout(Duration::from_secs(5), async {
            tokio::select! {
                index = acks.recv() => index.context("fanout readers stopped"),
                ended = jobs.join_next() => {
                    if let Some(job) = ended { job??; }
                    anyhow::bail!("fanout reader ended before final window")
                }
            }
        })
        .await
        .context("fanout window timed out")??;
        ensure!(
            index < seen.len() && !seen[index],
            "invalid fanout acknowledgement"
        );
        seen[index] = true;
    }
    Ok(())
}

    #[tokio::test(flavor = "current_thread")]
    #[ignore]
    async fn queued_acknowledgement_screen() {
        const READERS: usize = 8192;
        let (tx, mut rx) = mpsc::channel(READERS);
        let mut jobs = JoinSet::new();
        jobs.spawn(std::future::pending::<ReaderResult>());
        let mut seen = vec![false; READERS];
        let mut ready = Vec::with_capacity(READERS);
        for label in ["old", "batch", "batch", "old"] {
            let mut times = Vec::with_capacity(9);
            for batch in 0..10 {
                let mut elapsed = Duration::ZERO;
                for _ in 0..10 {
                    for index in 0..READERS { tx.send(index).await.unwrap(); }
                    let start = Instant::now();
                    if label == "old" {
                        acknowledge_old(&mut rx, &mut jobs, &mut seen).await.unwrap();
                    } else {
                        acknowledge(&mut rx, &mut jobs, &mut seen, &mut ready).await.unwrap();
                    }
                    elapsed += start.elapsed();
                    assert!(std::hint::black_box(&seen).iter().all(|&value| value));
                }
                if batch > 0 { times.push(elapsed.as_nanos() as f64 / 10.0); }
            }
            println!("{}", serde_json::json!({"variant":label,"readers":READERS,"ns_per_window":times}));
        }
    }
}
