use super::*;

#[test]
fn admin_token_bytes_follow_http_field_rules_without_restricting_relay_passwords() {
    for byte in 0u8..=127 {
        let value = format!("left{}right", char::from(byte));
        let representable = axum::http::HeaderValue::from_bytes(value.as_bytes()).is_ok();
        let config = Config {
            admin_token: Some(value.clone()),
            ..Config::default()
        };
        assert_eq!(
            config.validate().is_ok(),
            representable,
            "HTTP field byte {byte}"
        );
        Config {
            auth_password: Some(value),
            ..Config::default()
        }
        .validate()
        .unwrap();
    }
}

#[test]
fn admin_tokens_cannot_end_in_http_optional_whitespace() {
    for value in ["fixture ", "fixture\t"] {
        let error = Config {
            admin_token: Some(value.into()),
            ..Config::default()
        }
        .validate()
        .unwrap_err();
        assert!(error.to_string().contains("WAR_ADMIN_TOKEN"));
        assert!(!error.to_string().contains("fixture"));
    }
    for value in [
        "fixture",
        "two words",
        "inside\ttab",
        " \tleading",
        "unicode-π-世界",
        "trailing-\u{a0}",
    ] {
        Config {
            admin_token: Some(value.into()),
            ..Config::default()
        }
        .validate()
        .unwrap();
    }
}

#[test]
fn pin_workers_defaults_to_auto_and_keeps_the_bare_flag() {
    let parse = |args: &[&str]| {
        Config::try_parse_from(["war-relay"].iter().chain(args))
            .map(|config| config.pin_workers)
            .ok()
    };
    assert_eq!(Config::default().pin_workers, PinWorkers::Auto);
    assert_eq!(parse(&["--pin-workers"]), Some(PinWorkers::On));
    assert_eq!(
        parse(&["--pin-workers", "--workers", "2"]),
        Some(PinWorkers::On)
    );
    assert_eq!(parse(&["--pin-workers=true"]), Some(PinWorkers::On));
    assert_eq!(parse(&["--pin-workers=off"]), Some(PinWorkers::Off));
    assert_eq!(parse(&["--pin-workers=sometimes"]), None);
    let unpinned = |pin_workers, workers| {
        Config {
            pin_workers,
            ..Config::default()
        }
        .worker_cores(workers)
        .unwrap()
        .is_none()
    };
    assert!(unpinned(PinWorkers::Off, 1));
    // Auto never claims fewer CPUs than it was given, and never fails.
    let allowed = core_affinity::get_core_ids().map_or(0, |cores| cores.len());
    if allowed > 1 {
        assert!(unpinned(PinWorkers::Auto, allowed - 1));
        assert!(unpinned(PinWorkers::Auto, allowed + 1));
        assert!(
            Config {
                pin_workers: PinWorkers::On,
                ..Config::default()
            }
            .worker_cores(allowed + 1)
            .is_err()
        );
    }
}

#[test]
fn receive_arena_has_a_separate_backing_limit_and_page_hint() {
    let config = Config::try_parse_from([
        "war-relay",
        "--receive-arena-bytes",
        "131072",
        "--receive-arena-page-bytes",
        "32768",
    ])
    .unwrap();
    config.validate().unwrap();
    assert_eq!(config.receive_arena_bytes, 131072);
    assert_eq!(config.receive_arena_page_bytes, 32768);
    for (bytes, page) in [(1, 2), (0, 0), (1, usize::MAX)] {
        assert!(
            Config {
                receive_arena_bytes: bytes,
                receive_arena_page_bytes: page,
                ..Config::default()
            }
            .validate()
            .is_err()
        );
    }
}
