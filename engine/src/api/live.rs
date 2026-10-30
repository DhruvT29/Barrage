//! The live WebSocket: the whole table when a page connects, then the
//! downloads, then the table again each time it changes, with the lines
//! saying what changed, and the downloads again each time they or the data
//! each cable has used change.
//!
//! It follows the table itself, not the event channel. A speed, a public
//! address or an RTT arriving changes the table without any `LinkEvent`, and
//! the page has to show them all.

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use tokio::sync::watch;

use super::view::{LiveMessage, change_message, downloads_view, table_view};
use crate::fetch::{CableUse, DownloadStatus, Downloads};
use crate::links::Link;
use crate::links::watch::LinkTable;

/// What one client is sent, in order.
pub struct Feed {
    table: LinkTable,
    /// The table as this client last saw it, once it has seen one.
    shown: Option<Vec<Link>>,
    downloads: watch::Receiver<Vec<DownloadStatus>>,
    /// What each cable has received, sent with the downloads.
    used: watch::Receiver<Vec<CableUse>>,
    /// Whether this client has been sent the downloads yet.
    downloads_sent: bool,
    /// False once nothing can change the downloads any more.
    downloads_open: bool,
}

impl Feed {
    pub fn new(table: LinkTable, downloads: &Downloads) -> Self {
        Self::from_parts(table, downloads.status(), downloads.used())
    }

    fn from_parts(
        table: LinkTable,
        downloads: watch::Receiver<Vec<DownloadStatus>>,
        used: watch::Receiver<Vec<CableUse>>,
    ) -> Self {
        Self {
            table,
            shown: None,
            downloads,
            used,
            downloads_sent: false,
            downloads_open: true,
        }
    }

    /// The next message for this client: the whole table the first time,
    /// then the downloads, then a change each time the table moves and the
    /// downloads each time they do. `None` once the watcher has stopped.
    ///
    /// Whatever changes while the client is still busy with its last message
    /// is merged into the next one, so a slow client is never sent a backlog
    /// and costs the server no queue.
    ///
    /// Cancel safe: the only waits are for the table or the downloads to
    /// change, so a `select!` may drop this future without losing anything.
    pub async fn next(&mut self) -> Option<LiveMessage> {
        let Some(shown) = &self.shown else {
            let now = self.table.latest();
            let table = table_view(&now);
            self.shown = Some(now);
            return Some(LiveMessage::Table { table });
        };
        if !self.downloads_sent {
            self.downloads_sent = true;
            return Some(downloads_message(&mut self.downloads, &mut self.used));
        }
        loop {
            tokio::select! {
                // A change to the links first when both are waiting: the
                // downloads are still there for the next call.
                biased;
                open = self.table.changed() => {
                    if !open {
                        return None;
                    }
                    let now = self.table.latest();
                    let message = change_message(shown, &now);
                    self.shown = Some(now);
                    return Some(message);
                }
                // The downloads, or only the data used: bytes past a cut
                // are received but nothing written moves. Both senders
                // live as long as the downloads do.
                changed = self.downloads.changed(), if self.downloads_open => match changed {
                    Ok(()) => return Some(downloads_message(&mut self.downloads, &mut self.used)),
                    // Every sender is gone; the table still moves.
                    Err(_) => self.downloads_open = false,
                },
                changed = self.used.changed(), if self.downloads_open => match changed {
                    Ok(()) => return Some(downloads_message(&mut self.downloads, &mut self.used)),
                    Err(_) => self.downloads_open = false,
                },
            }
        }
    }
}

/// The downloads and the data used as they are now, both marked as seen.
fn downloads_message(
    downloads: &mut watch::Receiver<Vec<DownloadStatus>>,
    used: &mut watch::Receiver<Vec<CableUse>>,
) -> LiveMessage {
    LiveMessage::Downloads {
        downloads: downloads_view(&downloads.borrow_and_update(), &used.borrow_and_update()),
    }
}

/// `GET /api/live`.
pub(super) async fn upgrade(
    ws: WebSocketUpgrade,
    State(table): State<LinkTable>,
    State(downloads): State<Downloads>,
) -> Response {
    let feed = Feed::new(table, &downloads);
    ws.on_upgrade(move |socket| stream(socket, feed))
}

async fn stream(mut socket: WebSocket, mut feed: Feed) {
    tracing::debug!("live client connected");
    loop {
        tokio::select! {
            message = feed.next() => {
                let Some(message) = message else {
                    // The engine is stopping. A proper close lets the page say
                    // so, rather than guess at a broken connection.
                    let _ = socket.send(Message::Close(None)).await;
                    tracing::debug!("live client closed: the watcher stopped");
                    return;
                };
                let text = match serde_json::to_string(&message) {
                    Ok(text) => text,
                    Err(e) => {
                        tracing::error!(error = %e, "a live message did not serialise");
                        return;
                    }
                };
                if socket.send(Message::Text(text.into())).await.is_err() {
                    tracing::debug!("live client gone");
                    return;
                }
            }
            incoming = socket.recv() => match incoming {
                // Pages send nothing; the library answers pings by itself.
                Some(Ok(Message::Close(_)) | Err(_)) | None => {
                    tracing::debug!("live client left");
                    return;
                }
                Some(Ok(_)) => {}
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::api::view::UseView;

    fn used(cable: &str, bytes: u64) -> CableUse {
        CableUse {
            cable: cable.to_string(),
            bytes,
        }
    }

    /// The data used can change while no status does: bytes past a cut are
    /// received and thrown away, so nothing written moves. The page still
    /// hears of it.
    #[tokio::test]
    async fn a_change_to_the_data_used_sends_the_downloads_once() {
        let (_links, table) = watch::channel(Vec::new());
        let (publish, statuses) = watch::channel(Vec::new());
        let (counts, counted) = watch::channel(Vec::new());
        let mut feed = Feed::from_parts(LinkTable::of(table), statuses, counted);
        let Some(LiveMessage::Table { .. }) = feed.next().await else {
            panic!("expected the table first");
        };
        let Some(LiveMessage::Downloads { downloads }) = feed.next().await else {
            panic!("expected the downloads next");
        };
        assert_eq!(downloads.used, []);

        counts.send_replace(vec![used("Wi-Fi", 5_000)]);
        let next = tokio::time::timeout(Duration::from_secs(1), feed.next()).await;
        let Ok(Some(LiveMessage::Downloads { downloads })) = next else {
            panic!("expected the downloads again: {next:?}");
        };
        assert_eq!(
            downloads.used,
            [UseView {
                cable: "Wi-Fi".to_string(),
                bytes: 5_000
            }]
        );
        quiet(&mut feed).await;

        // Both at once, as a download publishes: one message carries both.
        counts.send_replace(vec![used("Wi-Fi", 9_000)]);
        publish.send_replace(Vec::new());
        let Some(LiveMessage::Downloads { downloads }) = feed.next().await else {
            panic!("expected the downloads");
        };
        assert_eq!(downloads.used[0].bytes, 9_000);
        quiet(&mut feed).await;
    }

    /// Nothing more until something changes again.
    async fn quiet(feed: &mut Feed) {
        let again = tokio::time::timeout(Duration::from_millis(200), feed.next()).await;
        assert!(again.is_err(), "sent again with nothing changed: {again:?}");
    }
}
