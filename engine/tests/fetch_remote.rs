//! Finding a file through one link: the ranged probe, redirects followed by
//! hand, and what the answer says about the file. Against the scripted
//! server in `serve/mod.rs`; no test here touches the internet.
//!
//! The requests leave from 127.0.0.2 rather than the server's own 127.0.0.1,
//! so the server's log shows that each one, every redirect hop included,
//! really left from the address it was asked to.

mod serve;

use std::net::IpAddr;
use std::time::Duration;

use barrage::fetch::{MAX_REDIRECTS, PROBE_BYTES, Validator, find};
use reqwest::Url;
use serve::{Answer, File, Server};

fn local() -> IpAddr {
    "127.0.0.2".parse().unwrap()
}

/// The probe's `Range`, as the server should see it.
fn probe_range() -> String {
    format!("bytes=0-{}", PROBE_BYTES - 1)
}

/// The whole error chain, as a note would show it.
async fn find_error(url: &Url) -> String {
    let err = find(local(), url)
        .await
        .expect_err("find should have failed");
    format!("{err:#}")
}

#[tokio::test]
async fn a_206_gives_the_length_and_says_ranges_work() {
    let server = Server::start().await;
    server.at("/big.bin", Answer::File(File::new(5_000_000)));

    let remote = find(local(), &server.url("/big.bin")).await.unwrap();
    assert_eq!(remote.length, Some(5_000_000));
    assert!(remote.ranges);
    assert_eq!(remote.url, server.url("/big.bin"));
    assert_eq!(remote.name, "big.bin");

    let seen = server.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].source, local());
    assert_eq!(seen[0].range.as_deref(), Some(probe_range().as_str()));
}

/// A file shorter than the probe comes back whole, and still as a 206.
#[tokio::test]
async fn a_file_shorter_than_the_probe_still_says_ranges_work() {
    let server = Server::start().await;
    server.at("/small.bin", Answer::File(File::new(500)));

    let remote = find(local(), &server.url("/small.bin")).await.unwrap();
    assert_eq!((remote.length, remote.ranges), (Some(500), true));
}

/// An empty file has no byte 0 to send, so the server answers the probe with
/// 416 and says the length is 0.
#[tokio::test]
async fn an_empty_file_is_found_with_length_0() {
    let server = Server::start().await;
    server.at("/empty", Answer::File(File::new(0)));

    let remote = find(local(), &server.url("/empty")).await.unwrap();
    assert_eq!((remote.length, remote.ranges), (Some(0), true));
}

#[tokio::test]
async fn a_200_means_no_ranges() {
    let server = Server::start().await;
    server.at("/whole", Answer::File(File::new(5_000_000).no_ranges()));
    let chunked = File::new(5_000_000).no_ranges().no_length();
    server.at("/chunked", Answer::File(chunked));

    let whole = find(local(), &server.url("/whole")).await.unwrap();
    assert_eq!((whole.length, whole.ranges), (Some(5_000_000), false));
    let chunked = find(local(), &server.url("/chunked")).await.unwrap();
    assert_eq!((chunked.length, chunked.ranges), (None, false));
}

/// A server that ignores the range sends the whole file. The probe takes the
/// headers and goes: this one sends one byte more than the probe asks for and
/// then holds the connection open, so a probe that read on would never
/// return.
#[tokio::test]
async fn the_probe_reads_no_more_than_the_headers_and_probe_bytes() {
    let server = Server::start().await;
    let stalls = File::new(50 << 20).no_ranges().stall_after(PROBE_BYTES + 1);
    server.at("/stalls", Answer::File(stalls));

    let url = server.url("/stalls");
    let found = tokio::time::timeout(Duration::from_secs(5), find(local(), &url));
    let remote = found.await.expect("the probe waited for the body").unwrap();
    assert_eq!(remote.length, Some(50 << 20));
}

#[tokio::test]
async fn head_is_never_sent() {
    let server = Server::start().await;
    server.at("/go", Answer::Redirect(302, "/file.bin".into()));
    server.at("/file.bin", Answer::File(File::new(10_000)));
    server.at("/whole.bin", Answer::File(File::new(10_000).no_ranges()));

    find(local(), &server.url("/go")).await.unwrap();
    find(local(), &server.url("/whole.bin")).await.unwrap();

    let seen = server.seen();
    assert_eq!(seen.len(), 3, "{seen:?}");
    assert!(seen.iter().all(|s| s.method.as_str() == "GET"), "{seen:?}");
}

#[tokio::test]
async fn redirects_are_followed_by_hand() {
    let server = Server::start().await;
    // Every redirect status, and every way of writing a Location.
    let hops = [
        ("/301", 301, "/302".to_string()),
        ("/302", 302, format!("{}/303", server.base())),
        ("/303", 303, "307".to_string()),
        ("/307", 307, format!("//{}/308", server.authority())),
        ("/308", 308, "/files/end.bin?sig=1".to_string()),
    ];
    for (path, status, location) in hops {
        server.at(path, Answer::Redirect(status, location));
    }
    server.at("/files/end.bin", Answer::File(File::new(4096)));

    let remote = find(local(), &server.url("/301")).await.unwrap();
    assert_eq!(remote.url, server.url("/files/end.bin?sig=1"));
    assert_eq!(remote.name, "end.bin");
    assert_eq!((remote.length, remote.ranges), (Some(4096), true));

    let seen = server.seen();
    let paths: Vec<&str> = seen.iter().map(|s| s.path.as_str()).collect();
    assert_eq!(
        paths,
        ["/301", "/302", "/303", "/307", "/308", "/files/end.bin"]
    );
    for hop in &seen {
        assert_eq!(hop.source, local(), "{hop:?}");
        assert_eq!(
            hop.range.as_deref(),
            Some(probe_range().as_str()),
            "{hop:?}"
        );
    }
}

#[tokio::test]
async fn too_many_redirects_is_an_error() {
    let server = Server::start().await;
    server.at("/loop", Answer::Redirect(302, "/loop".into()));

    let text = find_error(&server.url("/loop")).await;
    assert!(
        text.contains(&format!("{MAX_REDIRECTS} redirects")),
        "{text}"
    );
    // It stopped asking: the first request and MAX_REDIRECTS more.
    assert_eq!(server.seen().len(), MAX_REDIRECTS + 1);
}

#[tokio::test]
async fn exactly_max_redirects_are_still_followed() {
    let server = Server::start().await;
    for hop in 1..=MAX_REDIRECTS + 1 {
        let next = format!("/hop/{}", hop - 1);
        server.at(&format!("/hop/{hop}"), Answer::Redirect(302, next));
    }
    server.at("/hop/0", Answer::File(File::new(100)));

    let start = server.url(&format!("/hop/{MAX_REDIRECTS}"));
    let remote = find(local(), &start).await.unwrap();
    assert_eq!(remote.url, server.url("/hop/0"));

    let one_more = server.url(&format!("/hop/{}", MAX_REDIRECTS + 1));
    let text = find_error(&one_more).await;
    assert!(
        text.contains(&format!("{MAX_REDIRECTS} redirects")),
        "{text}"
    );
}

#[tokio::test]
async fn a_redirect_to_another_scheme_is_refused() {
    let server = Server::start().await;
    let elsewhere = [
        ("/ftp", "ftp://example.com/x"),
        ("/disk", "file:///C:/Windows/win.ini"),
    ];
    for (path, target) in elsewhere {
        server.at(path, Answer::Redirect(302, target.into()));
        let text = find_error(&server.url(path)).await;
        let refusal = format!("redirected to {target}, which is not an http or https address");
        assert!(text.contains(&refusal), "{text}");
    }
    // Nothing was asked of the place it pointed at.
    assert_eq!(server.seen().len(), 2);

    // Nor is such an address followed when it is the one given.
    let text = find_error(&Url::parse("ftp://example.com/x").unwrap()).await;
    let refusal = "ftp://example.com/x is not an http or https address";
    assert!(text.contains(refusal), "{text}");
}

#[tokio::test]
async fn a_redirect_without_a_location_is_an_error() {
    let server = Server::start().await;
    server.at("/nowhere", Answer::Status(302));

    let text = find_error(&server.url("/nowhere")).await;
    assert!(text.contains("Location"), "{text}");
}

#[tokio::test]
async fn the_validator_is_a_strong_etag_else_last_modified() {
    const DATE: &str = "Sat, 17 Oct 2026 10:00:00 GMT";
    let server = Server::start().await;
    let file = || File::new(10_000);
    server.at(
        "/strong",
        Answer::File(file().etag("\"v1\"").last_modified(DATE)),
    );
    server.at(
        "/weak",
        Answer::File(file().etag("W/\"v1\"").last_modified(DATE)),
    );
    server.at(
        "/unquoted",
        Answer::File(file().etag("v1").last_modified(DATE)),
    );
    server.at("/weak-only", Answer::File(file().etag("W/\"v1\"")));
    server.at("/date-only", Answer::File(file().last_modified(DATE)));
    server.at("/none", Answer::File(file()));

    let date = || Some(Validator::LastModified(DATE.into()));
    let cases = [
        ("/strong", Some(Validator::ETag("\"v1\"".into()))),
        // A weak tag cannot go in If-Range, and an unquoted one is no tag.
        ("/weak", date()),
        ("/unquoted", date()),
        ("/weak-only", None),
        ("/date-only", date()),
        ("/none", None),
    ];
    for (path, want) in cases {
        let remote = find(local(), &server.url(path)).await.unwrap();
        assert_eq!(remote.validator, want, "{path}");
    }
}

#[tokio::test]
async fn the_name_comes_from_the_answer_else_the_last_hop() {
    let server = Server::start().await;
    let named = |value: &str| Answer::File(File::new(10).disposition(value));
    server.at(
        "/latest",
        Answer::Redirect(302, "/store/setup-2.4.exe?t=1".into()),
    );
    server.at("/store/setup-2.4.exe", Answer::File(File::new(10)));
    server.at("/get", named("attachment; filename=\"report.pdf\""));
    // Raw UTF-8, as many servers send it, not the encoded form.
    server.at("/raw", named("attachment; filename=\"résumé.pdf\""));
    server.at("/climb", named("attachment; filename=\"../../evil.exe\""));

    let cases = [
        ("/latest", "setup-2.4.exe"),
        ("/get", "report.pdf"),
        ("/raw", "résumé.pdf"),
        ("/climb", "evil.exe"),
    ];
    for (path, want) in cases {
        let remote = find(local(), &server.url(path)).await.unwrap();
        assert_eq!(remote.name, want, "{path}");
    }
}

#[tokio::test]
async fn an_answer_that_is_not_the_file_is_an_error() {
    let server = Server::start().await;
    server.at("/gone", Answer::Status(404));
    server.at("/broken", Answer::Status(500));
    // Unsatisfiable, yet not said to be empty.
    server.at("/unsatisfiable", Answer::Status(416));

    for (path, status) in [("/gone", 404), ("/broken", 500), ("/unsatisfiable", 416)] {
        let text = find_error(&server.url(path)).await;
        assert!(text.contains(&format!("answered {status}")), "{text}");
    }
}

/// A 206 must hold the bytes asked for, or its pieces would land in the
/// wrong places later.
#[tokio::test]
async fn a_206_must_hold_the_bytes_asked_for() {
    let server = Server::start().await;
    let partial = |range: &str| Answer::Headers(206, vec![("content-range", range.into())]);
    server.at("/unknown-total", partial("bytes 0-1023/*"));
    server.at("/shifted", partial("bytes 100-1123/5000"));
    server.at("/late-start", partial("bytes 1-1023/5000"));
    server.at("/longer", partial("bytes 0-4999/5000"));
    server.at("/longer-unknown", partial("bytes 0-4999/*"));
    server.at("/past-the-end", partial("bytes 0-1023/500"));
    server.at("/garbled", partial("pages 1-2"));
    server.at("/unlabelled", Answer::Headers(206, vec![]));

    let remote = find(local(), &server.url("/unknown-total")).await.unwrap();
    assert_eq!((remote.length, remote.ranges), (None, true));

    for path in [
        "/shifted",
        "/late-start",
        "/longer",
        "/longer-unknown",
        "/past-the-end",
        "/garbled",
        "/unlabelled",
    ] {
        let text = find_error(&server.url(path)).await;
        assert!(text.contains("Content-Range"), "{path}: {text}");
    }
}
