//! Only this machine's own pages may use the API.
//!
//! Listening on 127.0.0.1 keeps other devices out, but not other websites. A
//! page from anywhere, open in this machine's browser, can open
//! `ws://127.0.0.1:<port>/api/live`, and a WebSocket gets none of the
//! cross-site protection the browser gives an ordinary request. That page
//! could read every link's public address, and later phases add controls.
//! Two checks close it:
//!
//! - **Host must name this machine**: `localhost` or a loopback address. A
//!   site that points its own name at 127.0.0.1 (DNS rebinding) is still sent
//!   its own name in Host.
//! - **Origin, when sent, must be that same host.** Browsers send it on
//!   WebSocket handshakes and cross-site requests. A request without one did
//!   not come from another site's page: an address typed into the browser, a
//!   test, curl.

use std::net::IpAddr;

use axum::extract::Request;
use axum::http::StatusCode;
use axum::http::header::{HOST, ORIGIN};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

/// Middleware: pass a request on only if [`from_this_machine`] says so;
/// answer anything else `403 Forbidden`.
pub async fn same_machine_only(request: Request, next: Next) -> Response {
    if allowed(&request) {
        next.run(request).await
    } else {
        (
            StatusCode::FORBIDDEN,
            "BARRAGE only answers its own pages on this machine.\n",
        )
            .into_response()
    }
}

/// Read outside the middleware's future: a request body is not `Sync`, so a
/// reference to the request held across its `.await` would make the future
/// unable to move between threads, which axum requires.
fn allowed(request: &Request) -> bool {
    let header = |name| request.headers().get(name)?.to_str().ok();
    from_this_machine(header(HOST), header(ORIGIN))
}

/// Whether a request with these `Host` and `Origin` headers came from this
/// machine's own pages. See the module docs for why both are checked.
pub fn from_this_machine(host: Option<&str>, origin: Option<&str>) -> bool {
    let Some(host) = host else {
        return false;
    };
    if !is_loopback_name(hostname(host)) {
        return false;
    }
    match origin {
        None => true,
        Some(origin) => origin
            .strip_prefix("http://")
            .is_some_and(|rest| rest.eq_ignore_ascii_case(host)),
    }
}

/// The name in a Host header, without its port: `localhost:7700` gives
/// `localhost`, `[::1]:7700` gives `::1`.
fn hostname(authority: &str) -> &str {
    match authority.strip_prefix('[') {
        Some(bracketed) => bracketed.split(']').next().unwrap_or_default(),
        None => authority.split(':').next().unwrap_or_default(),
    }
}

fn is_loopback_name(name: &str) -> bool {
    name.eq_ignore_ascii_case("localhost")
        || name.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_machines_names_are_accepted() {
        for host in [
            "127.0.0.1:7700",
            "localhost:7700",
            "LOCALHOST:7700",
            "[::1]:7700",
            "localhost",
        ] {
            assert!(from_this_machine(Some(host), None), "{host}");
        }
    }

    /// DNS rebinding sends the attacker's own name in Host. A LAN address
    /// is not this machine's loopback either: the server never listens there.
    #[test]
    fn any_other_host_is_refused() {
        for host in [
            "attacker.example:7700",
            "192.0.2.10:7700",
            "localhost.attacker.example:7700",
            "::1",
            "",
        ] {
            assert!(!from_this_machine(Some(host), None), "{host}");
        }
        assert!(!from_this_machine(None, None), "no Host at all");
    }

    /// The page the engine serves, and Vite's dev server, which proxies with
    /// the browser's own Host and Origin.
    #[test]
    fn a_page_from_the_same_host_is_accepted() {
        assert!(from_this_machine(
            Some("127.0.0.1:7700"),
            Some("http://127.0.0.1:7700")
        ));
        assert!(from_this_machine(
            Some("localhost:5173"),
            Some("http://localhost:5173")
        ));
    }

    #[test]
    fn a_page_from_anywhere_else_is_refused() {
        for origin in [
            "https://attacker.example",
            "http://127.0.0.1:9999",
            "https://127.0.0.1:7700",
            "null",
        ] {
            assert!(
                !from_this_machine(Some("127.0.0.1:7700"), Some(origin)),
                "{origin}"
            );
        }
    }
}
