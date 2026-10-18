//! Only this machine's own pages may use the API.
//!
//! Listening on 127.0.0.1 keeps other devices out, but not other websites. A
//! page from anywhere, open in this machine's browser, can make the browser
//! send requests here: open `ws://127.0.0.1:<port>/api/live`, which gets none
//! of the cross-site protection an ordinary request does; load `/api/links`
//! as an image or a script; or show the dashboard in a frame of its own.
//! That page could read every link's public address, and from phase 3 the
//! dashboard starts downloads. Three checks and one rule close it:
//!
//! - **Host must name this machine**: `localhost` or a loopback address. A
//!   site that points its own name at 127.0.0.1 (DNS rebinding) is still sent
//!   its own name in Host.
//! - **Origin, when sent, must be that same host.** Browsers send it on
//!   WebSocket handshakes and on requests a page makes itself, but not on an
//!   image, script or frame load, nor on a navigation. A request without one
//!   may still come from another site's page.
//! - **`Sec-Fetch-Site`, when sent, must be `same-origin` or `none`.** Chrome,
//!   Firefox and Safari (from 16.4) send it on every request, those loads and
//!   navigations included, naming where it came from. `same-origin` is this
//!   page, and `none` is no page at all: an address typed in, a bookmark, the
//!   browser `barrage` opens. `same-site` is a page on another port of this
//!   machine, since a site ignores the port, and `cross-site` is anywhere
//!   else. Any other value is refused until someone decides what it means.
//!   A request with neither header came from no browser page: curl, a test.
//!   An older browser that sends neither is the gap left, and its pages still
//!   cannot read an answer across sites.
//! - **No framing.** Every answer, refusals included, carries
//!   `X-Frame-Options: DENY` and `Content-Security-Policy: frame-ancestors
//!   'none'`, so no other page can show the dashboard inside its own and
//!   steer a click onto its buttons.

use std::net::IpAddr;

use axum::extract::Request;
use axum::http::header::{CONTENT_SECURITY_POLICY, HOST, ORIGIN, X_FRAME_OPTIONS};
use axum::http::{HeaderName, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

/// Not among the `http` crate's named headers.
const SEC_FETCH_SITE: HeaderName = HeaderName::from_static("sec-fetch-site");

/// Middleware: pass a request on only if [`from_this_machine`] says so, and
/// answer anything else `403 Forbidden`. Either way, forbid framing.
pub async fn same_machine_only(request: Request, next: Next) -> Response {
    let mut response = if allowed(&request) {
        next.run(request).await
    } else {
        (
            StatusCode::FORBIDDEN,
            "BARRAGE only answers its own pages on this machine.\n",
        )
            .into_response()
    };
    let headers = response.headers_mut();
    headers.insert(X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(
        CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("frame-ancestors 'none'"),
    );
    response
}

/// Read outside the middleware's future: a request body is not `Sync`, so a
/// reference to the request held across its `.await` would make the future
/// unable to move between threads, which axum requires.
fn allowed(request: &Request) -> bool {
    let header = |name: HeaderName| request.headers().get(name)?.to_str().ok();
    let (host, origin, fetch_site) = (header(HOST), header(ORIGIN), header(SEC_FETCH_SITE));
    let ok = from_this_machine(host, origin, fetch_site);
    if !ok {
        tracing::debug!(
            path = request.uri().path(),
            ?host,
            ?origin,
            ?fetch_site,
            "refused a request that is not from this machine's own pages"
        );
    }
    ok
}

/// Whether a request with these `Host`, `Origin` and `Sec-Fetch-Site`
/// headers came from this machine's own pages. See the module docs for why
/// each is checked.
pub fn from_this_machine(
    host: Option<&str>,
    origin: Option<&str>,
    fetch_site: Option<&str>,
) -> bool {
    let Some(host) = host else {
        return false;
    };
    if !is_loopback_name(hostname(host)) {
        return false;
    }
    let same_origin = match origin {
        None => true,
        Some(origin) => origin
            .strip_prefix("http://")
            .is_some_and(|rest| rest.eq_ignore_ascii_case(host)),
    };
    same_origin && fetch_site.is_none_or(|site| matches!(site, "same-origin" | "none"))
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
            assert!(from_this_machine(Some(host), None, None), "{host}");
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
            assert!(!from_this_machine(Some(host), None, None), "{host}");
        }
        assert!(!from_this_machine(None, None, None), "no Host at all");
    }

    /// The page the engine serves, and Vite's dev server, which proxies with
    /// the browser's own Host, Origin and fetch site.
    #[test]
    fn a_page_from_the_same_host_is_accepted() {
        assert!(from_this_machine(
            Some("127.0.0.1:7700"),
            Some("http://127.0.0.1:7700"),
            Some("same-origin")
        ));
        assert!(from_this_machine(
            Some("localhost:5173"),
            Some("http://localhost:5173"),
            Some("same-origin")
        ));
    }

    /// No Origin is sent on an image, script or frame load, or on a
    /// navigation, so it alone cannot tell. `Sec-Fetch-Site` can.
    #[test]
    fn another_sites_page_is_refused_even_without_an_origin() {
        assert!(!from_this_machine(
            Some("127.0.0.1:7700"),
            None,
            Some("cross-site")
        ));
        // A page on another port of this machine. A site ignores the port,
        // so the browser calls it the same site, but it is not this page.
        assert!(!from_this_machine(
            Some("127.0.0.1:7700"),
            None,
            Some("same-site")
        ));
    }

    #[test]
    fn a_typed_address_and_the_own_page_are_allowed() {
        // Typed into the address bar, a bookmark, the browser `barrage` opens.
        assert!(from_this_machine(
            Some("127.0.0.1:7700"),
            None,
            Some("none")
        ));
        assert!(from_this_machine(
            Some("127.0.0.1:7700"),
            Some("http://127.0.0.1:7700"),
            Some("same-origin")
        ));
        // Not a browser at all: curl, the tests.
        assert!(from_this_machine(Some("127.0.0.1:7700"), None, None));
    }

    /// Only the two values meaning "this page" and "no page" pass, so a value
    /// browsers add later is refused until someone decides what it means.
    #[test]
    fn an_unknown_fetch_site_is_refused() {
        for site in ["", "Same-Origin", "anything"] {
            assert!(
                !from_this_machine(Some("127.0.0.1:7700"), None, Some(site)),
                "{site:?}"
            );
        }
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
                !from_this_machine(Some("127.0.0.1:7700"), Some(origin), None),
                "{origin}"
            );
        }
    }
}
