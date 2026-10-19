use std::{hint::black_box, time::{Duration, Instant}};
mod timers;
fn main() {
    let args: Vec<_> = std::env::args().collect();
    let population: usize = args[1].parse().unwrap();
    let percent: usize = args[2].parse().unwrap();
    let base = Instant::now();
    let mut queue = timers::Timers::default();
    let mut deadlines: Vec<_> = (0..population).map(|slot| base + Duration::from_micros(slot as u64)).collect();
    for (slot, deadline) in deadlines.iter().enumerate() {
        queue.set(slot, Some(*deadline));
    }
    let start = Instant::now();
    for round in 0..24 {
        for n in 0..population {
            let slot = (n * 7919) % population;
            if n % 100 < percent {
                deadlines[slot] = base + Duration::from_micros((population + round * population + n) as u64);
            }
            queue.set(black_box(slot), Some(black_box(deadlines[slot])));
        }
    }
    let elapsed = start.elapsed();
    let mut expected: Vec<_> = deadlines.into_iter().enumerate().map(|(slot, deadline)| (deadline, slot)).collect();
    expected.sort_unstable();
    for (deadline, slot) in expected {
        assert_eq!(queue.next(), Some(deadline));
        assert_eq!(queue.pop_due(deadline), Some(slot));
    }
    assert!(queue.next().is_none());
    println!("{}", elapsed.as_nanos());
}
