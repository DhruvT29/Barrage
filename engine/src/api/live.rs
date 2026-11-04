//! The live WebSocket: the whole table when a page connects, then the
//! downloads, then the torrents, then the table again each time it changes,
//! with the lines saying what changed, the downloads again each time they or
//! the data each cable has used change, and the torrents each time they or
//! the session's UDP cable and port do.
//!
//! It follows the table itself, not the event channel. A speed, a public
//! address or an RTT arriving changes the table without any `LinkEvent`, and
//! the page has to show them all.

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use tokio::sync::watch;

use super::view::{LiveMessage, change_message, downloads_view, table_view, torrents_view};
use crate::bt::{Swarm, TorrentStatus, Torrents};
use crate::fetch::{CableUse, DownloadStatus, Downloads};
use crate::links::Link;
use crate::links::watch::LinkTable;
use crate::transfer::TransferEngine;

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
    torrents: watch::Receiver<Vec<TorrentStatus>>,
    /// The UDP cable and the listening port, sent with the torrents.
    swarm: watch::Receiver<Swarm>,
    /// Whether this client has been sent the torrents yet.
    torrents_sent: bool,
    /// False once nothing can change the torrents any more.
    torrents_open: bool,
}

impl Feed {
    pub fn new(table: LinkTable, downloads: &Downloads, torrents: &Torrents) -> Self {
        Self::from_parts(
            table,
            downloads.status(),
            downloads.used(),
            torrents.status(),
            torrents.swarm(),
        )
    }

    fn from_parts(
        table: LinkTable,
        downloads: watch::Receiver<Vec<DownloadStatus>>,
        used: watch::Receiver<Vec<CableUse>>,
        torrents: watch::Receiver<Vec<TorrentStatus>>,
        swarm: watch::Receiver<Swarm>,
    ) -> Self {
        Self {
            table,
            shown: None,
            downloads,
            used,
            downloads_sent: false,
            downloads_open: true,
            torrents,
            swarm,
            torrents_sent: false,
            torrents_open: true,
        }
    }

    /// The next message for this client: the whole table the first time,
    /// then the downloads, then the torrents, then a change each time the
    /// table moves and the downloads or torrents each time they do. `None`
    /// once the watcher has stopped.
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
        if !self.torrents_sent {
            self.torrents_sent = true;
            return Some(torrents_message(&mut self.torrents, &mut self.swarm));
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
                changed = self.torrents.changed(), if self.torrents_open => match changed {
                    Ok(()) => return Some(torrents_message(&mut self.torrents, &mut self.swarm)),
                    Err(_) => self.torrents_open = false,
                },
                changed = self.swarm.changed(), if self.torrents_open => match changed {
                    Ok(()) => return Some(torrents_message(&mut self.torrents, &mut self.swarm)),
                    Err(_) => self.torrents_open = false,
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

/// The torrents and the swarm as they are now, both marked as seen.
fn torrents_message(
    torrents: &mut watch::Receiver<Vec<TorrentStatus>>,
    swarm: &mut watch::Receiver<Swarm>,
) -> LiveMessage {
    LiveMessage::Torrents {
        torrents: torrents_view(&torrents.borrow_and_update(), &swarm.borrow_and_update()),
    }
}

/// `GET /api/live`.
pub(super) async fn upgrade(
    ws: WebSocketUpgrade,
    State(table): State<LinkTable>,
    State(downloads): State<Downloads>,
    State(torrents): State<Torrents>,
) -> Response {
    let feed = Feed::new(table, &downloads, &torrents);
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
    use crate::bt::{Listening, TorrentId, TorrentPhase};

    fn used(cable: &str, bytes: u64) -> CableUse {
        CableUse {
            cable: cable.to_string(),
            bytes,
            sent: 0,
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
        let (_torrents, torrents) = watch::channel(Vec::new());
        let (_swarm, swarm) = watch::channel(Swarm::default());
        let mut feed = Feed::from_parts(LinkTable::of(table), statuses, counted, torrents, swarm);
        let Some(LiveMessage::Table { .. }) = feed.next().await else {
            panic!("expected the table first");
        };
        let Some(LiveMessage::Downloads { downloads }) = feed.next().await else {
            panic!("expected the downloads next");
        };
        assert_eq!(downloads.used, []);
        let Some(LiveMessage::Torrents { .. }) = feed.next().await else {
            panic!("expected the torrents last");
        };

        counts.send_replace(vec![used("Wi-Fi", 5_000)]);
        let next = tokio::time::timeout(Duration::from_secs(1), feed.next()).await;
        let Ok(Some(LiveMessage::Downloads { downloads })) = next else {
            panic!("expected the downloads again: {next:?}");
        };
        assert_eq!(
            downloads.used,
            [UseView {
                cable: "Wi-Fi".to_string(),
                bytes: 5_000,
                sent: 0
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

    fn status(id: u64, done: u64) -> TorrentStatus {
        TorrentStatus {
            id: TorrentId(id),
            name: None,
            info_hash: "ab".repeat(20),
            phase: TorrentPhase::Downloading,
            length: None,
            done,
            down_bps: 0,
            up_bps: 0,
            peers: 0,
            note: None,
            cables: Vec::new(),
        }
    }

    /// The torrents or the swarm changing sends the torrents, once, the
    /// whole of both.
    #[tokio::test]
    async fn a_change_to_the_torrents_or_the_swarm_sends_the_torrents_once() {
        let (_links, table) = watch::channel(Vec::new());
        let (_statuses, statuses) = watch::channel(Vec::new());
        let (_counts, counted) = watch::channel(Vec::new());
        let (publish, torrents) = watch::channel(Vec::new());
        let (moved, swarm) = watch::channel(Swarm::default());
        let mut feed = Feed::from_parts(LinkTable::of(table), statuses, counted, torrents, swarm);
        for _ in 0..3 {
            feed.next().await;
        }
        quiet(&mut feed).await;

        publish.send_replace(vec![status(1, 700)]);
        let Some(LiveMessage::Torrents { torrents }) = sent(&mut feed).await else {
            panic!("expected the torrents");
        };
        assert_eq!(torrents.list[0].done, 700);
        quiet(&mut feed).await;

        moved.send_replace(Swarm {
            udp_cable: Some("Wi-Fi".to_string()),
            listening: Listening::Open {
                cable: "Wi-Fi".to_string(),
            },
        });
        let Some(LiveMessage::Torrents { torrents }) = sent(&mut feed).await else {
            panic!("expected the torrents");
        };
        assert_eq!(torrents.udp_cable.as_deref(), Some("Wi-Fi"));
        assert_eq!(torrents.listening.state, "open");
        assert_eq!(torrents.list.len(), 1);
        quiet(&mut feed).await;

        // Both at once: one message carries both.
        publish.send_replace(vec![status(1, 900)]);
        moved.send_replace(Swarm::default());
        let Some(LiveMessage::Torrents { torrents }) = sent(&mut feed).await else {
            panic!("expected the torrents");
        };
        assert_eq!(torrents.list[0].done, 900);
        assert_eq!(torrents.listening.state, "off");
        quiet(&mut feed).await;
    }

    /// The next message, or `None` if none comes within a second.
    async fn sent(feed: &mut Feed) -> Option<LiveMessage> {
        tokio::time::timeout(Duration::from_secs(1), feed.next())
            .await
            .ok()
            .flatten()
    }

    /// Nothing more until something changes again.
    async fn quiet(feed: &mut Feed) {
        let again = tokio::time::timeout(Duration::from_millis(200), feed.next()).await;
        assert!(again.is_err(), "sent again with nothing changed: {again:?}");
    }
}
