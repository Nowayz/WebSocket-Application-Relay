from pathlib import Path
p=Path('examples/capacity.rs');s=p.read_text()
s=s.replace('''    user_latency_output: Option<std::path::PathBuf>,
''','''    user_latency_output: Option<std::path::PathBuf>,
    /// Bounded per-user histogram CSV for long runs; percentile columns are bounds.
    #[arg(long, conflicts_with = "user_latency_output")]
    user_latency_histogram_output: Option<std::path::PathBuf>,
''')
a=s.index('    fn report(&self) -> serde_json::Value {');b=s.index('\nstruct LatencySample',a)
old=s[a:b];new='''    fn percentile_upper_ms(&self, p: u64) -> f64 {
        if self.n == 0 {
            return 0.0;
        }
        let rank = (self.n * p).div_ceil(100);
        let mut count = 0;
        for (bucket, n) in self.buckets.iter().enumerate() {
            count += n;
            if count >= rank {
                return (1u128 << (bucket + 1)) as f64 / 1e6;
            }
        }
        unreachable!()
    }
    fn report(&self) -> serde_json::Value {
        serde_json::json!({"samples":self.n,"p50_upper_ms":self.percentile_upper_ms(50),
            "p99_upper_ms":self.percentile_upper_ms(99),"max_ms":self.maximum as f64/1e6})
    }
}

#[derive(Default)]
struct UserLatencyHistogram {
    send: Histogram,
    scheduled: Histogram,
    pings: u64,
}

impl UserLatencyHistogram {
    fn record(&mut self, send: Duration, scheduled: Duration, pings: u64) {
        self.send.record(send);
        self.scheduled.record(scheduled);
        self.pings += pings;
    }
}
''';s=s[:a]+new+s[b:]
s=s.replace('''    latency: Option<Vec<LatencySample>>,
''','''    latency: Option<Vec<LatencySample>>,
    histogram: Option<Box<UserLatencyHistogram>>,
''')
s=s.replace('args.user_latency_output.is_none() || args.waves == 1','(args.user_latency_output.is_none() && args.user_latency_histogram_output.is_none()) || args.waves == 1')
s=s.replace('''            latency: None,
''','''            latency: None,
            histogram: None,
''')
s=s.replace('''    let diagnostic = room.peers[0].latency.is_some();''','''    let diagnostic = room.peers[0].latency.is_some() || room.peers[0].histogram.is_some();''')
s=s.replace('''    if room.peers[0].latency.is_some() {
        let mut receives''','''    if diagnostic {
        let mut receives''')
s=s.replace('''pings, &mut peer.latency))''','''pings, &mut peer.latency, &mut peer.histogram))''')
s=s.replace('''let (member, packet, received, pings, samples) = received?;''','''let (member, packet, received, pings, samples, histogram) = received?;''')
a=s.index('''            samples
                .as_mut()
                .expect("diagnostic samples")''');b=s.index('''            observations.round_trip.record(latency);''',a)
s=s[:a]+'''            let scheduled = received.duration_since(observations.scheduled.unwrap_or(started));
            if let Some(samples) = samples {
                samples.push(LatencySample {
                    send_ns: latency.as_nanos() as u64,
                    scheduled_ns: scheduled.as_nanos() as u64,
                    pings,
                });
            }
            if let Some(histogram) = histogram {
                histogram.record(latency, scheduled, pings);
            }
'''+s[b:]
s=s.replace('''    let (checked, proof) = prove_rooms(rooms, 0, args.payload_bytes, args.concurrency).await?;
    merge(&mut total, proof);''','''    let (mut checked, proof) = prove_rooms(rooms, 0, args.payload_bytes, args.concurrency).await?;
    merge(&mut total, proof);
    if args.user_latency_histogram_output.is_some() {
        let active_rooms = (room_count * args.active_percent).div_ceil(100);
        for room in &mut checked {
            if room.index < active_rooms {
                for peer in &mut room.peers {
                    peer.histogram = Some(Box::default());
                }
            }
        }
    }''')
mark='''    // Idle heartbeat responses alone do not prove application routing survived.''';assert mark in s
s=s.replace(mark,'''    if let Some(path) = &args.user_latency_histogram_output {
        use std::io::BufWriter;
        let mut output = BufWriter::new(std::fs::File::create(path)?);
        writeln!(output, "room,member,id,samples,p50_upper_ms,p99_upper_ms,max_ms,schedule_p99_upper_ms,schedule_max_ms,pings")?;
        for room in &mut rooms {
            for (member, peer) in room.peers.iter_mut().enumerate() {
                let Some(histogram) = peer.histogram.take() else { continue };
                writeln!(output, "{},{},{},{},{},{},{},{},{},{}", room.index, member, peer.id,
                    histogram.send.n, histogram.send.percentile_upper_ms(50),
                    histogram.send.percentile_upper_ms(99), histogram.send.maximum as f64 / 1e6,
                    histogram.scheduled.percentile_upper_ms(99), histogram.scheduled.maximum as f64 / 1e6,
                    histogram.pings)?;
            }
        }
        output.flush()?;
    }

'''+mark)
mark='''    #[test]
    fn exact_timeline_quantiles''';s=s.replace(mark,'''    #[test]
    fn bounded_user_histograms_keep_separate_clocks_counts_and_ping_totals() {
        let mut histogram = UserLatencyHistogram::default();
        for millis in [1, 2, 3, 100] {
            histogram.record(Duration::from_millis(millis), Duration::from_millis(millis + 200), 2);
        }
        assert_eq!(histogram.send.n, 4);
        assert_eq!(histogram.scheduled.n, 4);
        assert_eq!(histogram.pings, 8);
        assert_eq!(histogram.send.maximum, 100_000_000);
        assert_eq!(histogram.send.percentile_upper_ms(99), 134.217728);
        assert_eq!(histogram.scheduled.percentile_upper_ms(99), 536.870912);
        assert_eq!(histogram.send.buckets.iter().sum::<u64>(), 4);
        assert_eq!(UserLatencyHistogram::default().send.percentile_upper_ms(99), 0.0);
    }

'''+mark)
p.write_text(s)
