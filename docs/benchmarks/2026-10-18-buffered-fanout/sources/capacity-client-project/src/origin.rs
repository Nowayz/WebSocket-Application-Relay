//! Optional browser-origin admission, evaluated once during HTTP upgrade.
use anyhow::{Context, Result, ensure};
use clap::Args;
use tokio_tungstenite::tungstenite::{
    handshake::server::ErrorResponse,
    http::{HeaderMap, Response, Uri, header},
};

/// Exact serialized browser origins. Missing Origin remains allowed for native
/// clients; this browser boundary is not authentication of non-browser clients.
#[derive(Clone, Default, Args)]
pub struct OriginPolicy {
    /// Allowed browser Origin (repeat or comma-separate); unset allows all.
    #[arg(
        long = "allowed-origin",
        env = "WAR_ALLOWED_ORIGINS",
        value_delimiter = ','
    )]
    pub allowed: Vec<String>,
}

impl OriginPolicy {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.allowed.len() <= 64,
            "at most 64 allowed origins are supported"
        );
        for origin in &self.allowed {
            ensure!(
                !origin.is_empty() && origin.len() <= 1024,
                "allowed origins must contain 1..=1024 bytes"
            );
            if origin == "null" {
                continue;
            }
            let uri: Uri = origin
                .parse()
                .context("allowed-origin must be http(s)://host[:port] or null")?;
            let authority = uri.authority().context("allowed-origin needs a host")?;
            let scheme = uri
                .scheme_str()
                .context("allowed-origin needs http or https")?;
            let suffix = &authority.as_str()[authority.host().len()..];
            ensure!(
                matches!(scheme, "http" | "https")
                    && !authority.host().is_empty()
                    && !authority.as_str().contains('@')
                    && !authority.host().contains('*')
                    && (suffix.is_empty()
                        || suffix.strip_prefix(':').is_some_and(|port| {
                            !port.is_empty()
                                && port.bytes().all(|b| b.is_ascii_digit())
                                && port.parse::<u16>().is_ok()
                        }))
                    && format!("{scheme}://{authority}") == *origin,
                "allowed-origin must be an exact http(s) origin without credentials, wildcard, path, query or fragment"
            );
        }
        Ok(())
    }

    /// Borrow header bytes and compare to the bounded configuration. No parsing
    /// or allocation is needed per request; no policy work occurs per message.
    pub fn allows(&self, headers: &HeaderMap) -> bool {
        if self.allowed.is_empty() {
            return true;
        }
        let mut origins = headers.get_all(header::ORIGIN).iter();
        let Some(origin) = origins.next() else {
            return true;
        };
        origins.next().is_none()
            && self
                .allowed
                .iter()
                .any(|allowed| allowed.as_bytes() == origin.as_bytes())
    }
}

pub(crate) fn forbidden() -> ErrorResponse {
    const BODY: &str = "Origin Not Allowed\n";
    Response::builder()
        .status(403)
        .header(header::CONNECTION, "close")
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .header(header::CONTENT_LENGTH, BODY.len())
        .body(Some(BODY.into()))
        .expect("static origin rejection")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_tungstenite::tungstenite::http::HeaderValue;

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
}
