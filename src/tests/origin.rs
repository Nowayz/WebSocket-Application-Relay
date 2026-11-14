use super::*;
use http::HeaderValue;

#[test]
fn exact_origins_preserve_native_clients_and_reject_ambiguous_values() {
    let policy = OriginPolicy {
        allowed: vec!["https://app.example".into(), "http://[::1]:4180".into()],
    };
    policy.validate().unwrap();
    let mut headers = HeaderMap::new();
    assert!(policy.allows(&headers));
    for origin in ["https://app.example", "http://[::1]:4180"] {
        headers.insert(header::ORIGIN, origin.parse().unwrap());
        assert!(policy.allows(&headers));
    }
    for origin in [
        "",
        "null",
        "https://app.example.evil",
        "https://app.example/",
        "http://app.example",
        "https://app.example:444",
        "https://app.example https://evil.example",
        "https://app.example,https://evil.example",
    ] {
        headers.insert(header::ORIGIN, origin.parse().unwrap());
        assert!(!policy.allows(&headers), "{origin}");
    }
    headers.insert(
        header::ORIGIN,
        HeaderValue::from_static("https://app.example"),
    );
    headers.append(
        header::ORIGIN,
        HeaderValue::from_static("https://app.example"),
    );
    assert!(!policy.allows(&headers));
    assert!(OriginPolicy::default().allows(&headers));
    headers.clear();
    headers.insert(header::ORIGIN, HeaderValue::from_static("null"));
    assert!(
        OriginPolicy {
            allowed: vec!["null".into()]
        }
        .allows(&headers)
    );
}

#[test]
fn configuration_rejects_non_origin_urls_and_unbounded_lists() {
    for origin in [
        "",
        "*",
        "https://*.example",
        "ws://app.example",
        "https://user@app.example",
        "https://app.example/",
        "https://app.example/path",
        "https://app.example?q=x",
        "https://app.example#x",
        "https://app.example:",
        "https://app.example:65536",
        "https://app.example:port",
        "http://:80",
        "file:///tmp",
        " https://app.example",
        "https://app.example https://other.example",
    ] {
        assert!(
            OriginPolicy {
                allowed: vec![origin.into()]
            }
            .validate()
            .is_err(),
            "{origin}"
        );
    }
    assert!(
        OriginPolicy {
            allowed: vec!["https://app.example".into(); 65]
        }
        .validate()
        .is_err()
    );
    assert!(
        OriginPolicy {
            allowed: vec!["x".repeat(1025)]
        }
        .validate()
        .is_err()
    );
}

#[test]
fn native_cli_accepts_repeated_and_comma_separated_origins() {
    use clap::Parser;
    let config = crate::Config::try_parse_from([
        "war-relay",
        "--allowed-origin",
        "https://app.example,http://localhost:4180",
        "--allowed-origin",
        "null",
    ])
    .unwrap();
    config.validate().unwrap();
    assert_eq!(
        config.origin_policy.allowed,
        ["https://app.example", "http://localhost:4180", "null"]
    );
}
