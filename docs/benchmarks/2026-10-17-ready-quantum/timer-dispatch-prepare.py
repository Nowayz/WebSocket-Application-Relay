from pathlib import Path
p=Path('src/server.rs');s=p.read_text();Path('artifacts/timer-dispatch-original-server.rs').write_text(s)
s=s.replace('let mut incoming = relay.receive_batch();\n        loop {','let mut incoming = relay.receive_batch();\n        let mut timer_hist = [0u64; 32];\n        let mut ready_hist = [0u64; 32];\n        let mut max_timer_ns = 0u128;\n        let mut max_ready_ns = 0u128;\n        let mut max_due = 0usize;\n        loop {')
s=s.replace('while let Some(slot) = self.timers.pop_due(now) {','let dispatch_start = Instant::now();\n            let mut due_count = 0usize;\n            while let Some(slot) = self.timers.pop_due(now) {\n                due_count += 1;')
s=s.replace('// A bounded ready batch makes progress','let timer_ns = dispatch_start.elapsed().as_nanos();\n            max_timer_ns = max_timer_ns.max(timer_ns);\n            max_due = max_due.max(due_count);\n            timer_hist[((128 - timer_ns.max(1).leading_zeros()) as usize).min(31)] += 1;\n            let ready_start = Instant::now();\n            // A bounded ready batch makes progress')
s=s.replace('// Publish before polling, even when no more input will arrive.','let ready_ns = ready_start.elapsed().as_nanos();\n            max_ready_ns = max_ready_ns.max(ready_ns);\n            ready_hist[((128 - ready_ns.max(1).leading_zeros()) as usize).min(31)] += 1;\n            // Publish before polling, even when no more input will arrive.')
needle='''        Ok(())
    }
}

struct ConnectionGuard'''
replace='''        eprintln!("DISPATCH_DIAGNOSTIC {:?} timer_max_ns={} ready_max_ns={} max_due={} timer_hist={:?} ready_hist={:?}", std::thread::current().name(), max_timer_ns, max_ready_ns, max_due, timer_hist, ready_hist);
        Ok(())
    }
}

struct ConnectionGuard'''
assert needle in s;s=s.replace(needle,replace);p.write_text(s)
