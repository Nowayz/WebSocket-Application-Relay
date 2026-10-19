#![cfg(unix)]

use std::{
    ffi::OsString,
    net::TcpListener,
    os::unix::ffi::OsStringExt,
    process::{Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

fn startup(secrets: &[(&str, OsString)]) -> Output {
    // Force an ordinary bind error if secret loading succeeds. Neither the
    // original nor selected executable can open a relay during these checks.
    let occupied = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_war-relay"));
    command
        .env_clear()
        .arg("--bind")
        .arg(occupied.local_addr().unwrap().to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (name, value) in secrets {
        command.env(name, value);
    }
    let mut child = command.spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("configuration check must finish before opening a listener");
        }
        thread::sleep(Duration::from_millis(5));
    }
    child.wait_with_output().unwrap()
}

#[test]
fn malformed_secret_encoding_is_rejected_before_binding_without_echoing_bytes() {
    for name in ["WAR_AUTH_PASSWORD", "WAR_ADMIN_TOKEN"] {
        let malformed = OsString::from_vec(b"redaction-fixture-\xff-secret".to_vec());
        let output = startup(&[(name, malformed)]);
        assert_eq!(output.status.code(), Some(1));
        let diagnostic = String::from_utf8_lossy(&output.stderr);
        assert!(
            diagnostic.contains(name),
            "must identify the invalid variable"
        );
        assert!(
            diagnostic.contains("UTF-8"),
            "must report the encoding error"
        );
        assert!(
            !diagnostic.contains("redaction-fixture"),
            "must not echo secret bytes"
        );
        assert!(!diagnostic.contains("cannot bind relay listener"));
        assert!(output.stdout.is_empty());
    }
}

#[test]
fn absent_empty_ascii_and_unicode_secrets_still_pass_configuration() {
    for value in [None, Some(""), Some("valid-fixture"), Some("valid-π-世界")] {
        let secrets: Vec<_> = value
            .map(|value| {
                ["WAR_AUTH_PASSWORD", "WAR_ADMIN_TOKEN"]
                    .into_iter()
                    .map(|name| (name, OsString::from(value)))
                    .collect()
            })
            .unwrap_or_default();
        let output = startup(&secrets);
        assert_eq!(output.status.code(), Some(1));
        let diagnostic = String::from_utf8_lossy(&output.stderr);
        assert!(diagnostic.contains("cannot bind relay listener"));
        assert!(!diagnostic.contains("valid-fixture"));
        assert!(!diagnostic.contains("valid-π-世界"));
    }
}
