//! Transport-independent relay IDs, listener flags, and close reasons.
pub const BROADCAST: u64 = u64::MAX;
pub const RELAY: u64 = 0;
pub const GLOBAL_CHANNEL: &[u8] = b"re_globl";
pub const CHANNEL_MESSAGE: u8 = 1;
pub const PRIVATE_MESSAGE: u8 = 2;
pub const DISCONNECT_MESSAGE: u8 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Disconnect {
    pub code: u16,
    pub reason: &'static str,
}

impl Disconnect {
    pub const PROTOCOL: Self = Self {
        code: 1002,
        reason: "Protocol Violation",
    };
    pub const CHANNEL_LENGTH: Self = Self {
        code: 1002,
        reason: "Channel Length Exceeded",
    };
    pub const OVERLOADED: Self = Self {
        code: 1013,
        reason: "Try Again Later",
    };
    pub const TAKEN: Self = Self {
        code: 4001,
        reason: "UserID Taken",
    };
    pub const SHUTDOWN: Self = Self {
        code: 1001,
        reason: "Server Shutdown",
    };
    pub const IDLE: Self = Self {
        code: 1001,
        reason: "Application Idle",
    };
    pub const RATE: Self = Self {
        code: 1008,
        reason: "Rate Limit Exceeded",
    };
    pub const RESERVED: Self = Self {
        code: 1008,
        reason: "Reserved UserID",
    };
}

pub fn secret_matches(expected: Option<&str>, actual: &[u8]) -> bool {
    use subtle::ConstantTimeEq;
    expected.is_some_and(|secret| bool::from(secret.as_bytes().ct_eq(actual)))
}
