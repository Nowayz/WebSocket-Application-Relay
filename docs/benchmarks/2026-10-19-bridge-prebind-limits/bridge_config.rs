#![cfg(unix)]

use std::{
    net::TcpListener,
    process::{Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

fn startup(connections: usize, per_ip: usize) -> Output {
    let occupied = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_war-legacy-bridge"))
        .env_clear()
        .env("TOKIO_WORKER_THREADS", "1")
        .arg("--bind")
        .arg(occupied.local_addr().unwrap().to_string())
        .arg("--max-connections")
        .arg(connections.to_string())
        .arg("--max-connections-per-ip")
        .arg(per_ip.to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("bridge configuration check must finish promptly");
        }
        thread::sleep(Duration::from_millis(5));
    }
    child.wait_with_output().unwrap()
}

#[test]
fn invalid_global_connection_limits_are_reported_before_binding() {
    for limit in [0, usize::MAX] {
        let output = startup(limit, 0);
        assert_eq!(output.status.code(), Some(1));
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(error.contains("invalid bridge connection limit"));
        assert!(!error.contains("cannot bind legacy bridge"));
        assert!(output.stdout.is_empty());
    }
}

#[cfg(target_pointer_width = "64")]
#[test]
fn invalid_per_ip_connection_limit_is_reported_before_binding() {
    let output = startup(1, u32::MAX as usize + 1);
    assert_eq!(output.status.code(), Some(1));
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("max-connections-per-ip must be in 0..=4294967295"));
    assert!(!error.contains("cannot bind legacy bridge"));
    assert!(output.stdout.is_empty());
}

#[test]
fn valid_admission_boundaries_still_reach_binding_without_allocating_slots() {
    for (connections, per_ip) in [
        (1, 0),
        (100_000, 0),
        (tokio::sync::Semaphore::MAX_PERMITS, u32::MAX as usize),
    ] {
        let output = startup(connections, per_ip);
        assert_eq!(output.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&output.stderr).contains("cannot bind legacy bridge"));
    }
}
