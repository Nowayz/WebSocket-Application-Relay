use super::*;

fn check(rates: [u64; 2], cases: &[(u64, usize, bool)]) {
    let now = Instant::now();
    let mut limiter = RateLimiter::new(rates[0], rates[1], now);
    for &(millis, bytes, expected) in cases {
        assert_eq!(
            limiter.allow(bytes, now + Duration::from_millis(millis)),
            expected,
            "rates={rates:?}, millis={millis}, bytes={bytes}"
        );
    }
}

#[test]
fn disabled_limits_admit_without_a_token_budget() {
    check(
        [0, 0],
        &[(0, 0, true), (0, usize::MAX, true), (500, usize::MAX, true)],
    );
}

#[test]
fn message_limit_refills_fractionally_and_caps_idle_credit() {
    check(
        [2, 0],
        &[
            (0, usize::MAX, true),
            (0, 1, true),
            (0, 1, false),
            (250, 1, false),
            (500, 1, true),
            (10000, 1, true),
            (10000, 1, true),
            (10000, 1, false),
        ],
    );
}

#[test]
fn byte_limit_charges_exact_sizes_and_ignores_message_count() {
    check(
        [0, 8],
        &[
            (0, 3, true),
            (0, 5, true),
            (0, 1, false),
            (125, 1, true),
            (125, 2, false),
            (10000, 8, true),
            (10000, 1, false),
        ],
    );
}

#[test]
fn coupled_limits_keep_the_existing_debit_order() {
    check(
        [2, 4],
        &[
            (0, 3, true),
            (0, 2, false),
            (500, 2, true),
            (500, 0, false),
            (1000, 4, false),
            (2000, 4, true),
            (2000, 0, true),
            (2000, 0, false),
        ],
    );
}
