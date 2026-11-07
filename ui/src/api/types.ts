// The engine's JSON, field for field, as `engine/src/api/view.rs` writes it.
// contract.ts holds messages the engine really sent, written by its own test:
// the type check holds them to these types and contract.test.tsx renders
// them, so a field changed on one side only fails the build or the tests.
// After changing view.rs, refresh contract.ts as its header says.

/** The engine's state words, the same ones `brg-links` prints. */
export type LinkState = 'probing' | 'up' | 'portal' | 'degraded' | 'down'

/** One link: one address on one interface. */
export interface LinkView {
  id: number
  /** The interface. Links with the same name share one cable. */
  name: string
  address: string
  family: 'ipv4' | 'ipv6'
  state: LinkState
  /** Whole milliseconds; null until probed. */
  rtt_ms: number | null
  /** The cable's measured speed in bits per second; null until measured, never 0 for unknown. */
  down_bps: number | null
  public_ip: string | null
  /** Whether Windows says the connection it is on is metered. */
  metered: boolean
}

/** The whole table, in the order links appeared. */
export interface TableView {
  links: LinkView[]
  /** Groups of link ids on different cables that share one internet connection. */
  shared: number[][]
  /** The cables on each shared connection, by name: joined directly or through another cable, measured or not. */
  connections: string[][]
  combined: CombinedView
}

/** The combined speed available, worked out by the engine. */
export interface CombinedView {
  /** The sum of the parts, in bits per second; 0 only when there are no parts. */
  bps: number
  /** One per independent connection, in the order its first cable appeared. */
  parts: CombinedPart[]
  /** The cables left out of the sum, and why. */
  not_counted: NotCounted[]
}

export interface CombinedPart {
  /** The connection's fastest cable. */
  name: string
  bps: number
  /** The other measured cables on the same connection, counted once here. */
  shared_with: string[]
}

export interface NotCounted {
  name: string
  why: 'not measured yet' | 'portal' | 'probing' | 'down'
}

/** A download's state words. `stopped` keeps what it saved, to resume from. */
export type DownloadState = 'queued' | 'starting' | 'waiting' | 'downloading' | 'done' | 'failed' | 'stopped'

/** Who goes next: high first, then the earliest added. */
export type Priority = 'high' | 'normal' | 'low'

/** Every download on the engine's list, and the speed they make together. */
export interface DownloadsView {
  live: LiveSpeed
  /** In the order they were added. */
  list: DownloadView[]
  /** What each cable has moved for every download and torrent since the data used was last zeroed, in the order cables first carried. */
  used: UseView[]
}

/** The data one cable has moved, in bytes. */
export interface UseView {
  cable: string
  /** Received. */
  bytes: number
  sent: number
}

/** What the downloads that are downloading fetch right now, all together. */
export interface LiveSpeed {
  /** The sum of the parts, in bits per second. */
  bps: number
  /** One per cable carrying data, in the order it first appears in the list. */
  parts: SpeedPart[]
}

export interface SpeedPart {
  name: string
  bps: number
}

/** One download. */
export interface DownloadView {
  id: number
  /** The URL as it was given. */
  url: string
  /** The file's name in the download folder; null until the file is found. */
  name: string | null
  state: DownloadState
  /** In bytes; null while the server has not said. */
  length: number | null
  /** Bytes written. */
  done: number
  /** In bits per second. */
  bps: number
  /** Why it failed, or what to know about it, in the engine's words. */
  note: string | null
  cables: CableShare[]
  priority: Priority
}

/** One cable's part in a download: its addresses' shares added up by the engine. */
export interface CableShare {
  name: string
  bytes: number
  /** In bits per second, 0 unless downloading. */
  bps: number
  connections: number
  /** The connections the download wants it to hold, by what it delivers: `connections` moves toward it one at a time. */
  wanted: number
  /** Each of its addresses' notes, each said once. */
  notes: string[]
}

/** A torrent's phase words. `done` and `stopped` share nothing. */
export type TorrentPhase = 'queued' | 'finding' | 'downloading' | 'done' | 'stopped' | 'failed'

/** Every torrent on the engine's list, and what the session does apart from any one of them. */
export interface TorrentsView {
  /** In the order they were added. */
  list: TorrentView[]
  /** The cable carrying DHT, UDP trackers and uTP; null while no cable works. */
  udp_cable: string | null
  listening: ListeningView
}

/** Whether peers can call in. */
export interface ListeningView {
  state: 'off' | 'trying' | 'open' | 'closed'
  cable: string | null
  /** Why it closed, in the engine's words. */
  why: string | null
}

/** One torrent. */
export interface TorrentView {
  id: number
  /** Its name, once known. */
  name: string | null
  /** 40 lowercase hex characters. */
  info_hash: string
  phase: TorrentPhase
  /** In bytes, once the metadata is known. */
  length: number | null
  /** Bytes downloaded and checked. */
  done: number
  /** In bits per second, 0 unless downloading. */
  down_bps: number
  up_bps: number
  /** Peers connected right now. */
  peers: number
  /** Why it failed, or what to know about it, in the engine's words. */
  note: string | null
  cables: TorrentCableView[]
  priority: Priority
}

/** One cable's part in a torrent. */
export interface TorrentCableView {
  name: string
  received: number
  sent: number
  /** In bits per second. */
  bps: number
  connections: number
  /** Whether its connections may upload (not on a metered connection). */
  uploads: boolean
}

/** One transfer's place in line, from 1. */
export interface PlaceView {
  kind: 'web' | 'torrent'
  id: number
  place: number
}

/** The line and the limit. */
export interface QueueView {
  /** How many run at once. */
  limit: number
  /** The highest limit the engine takes. */
  most: number
  /** How many hold a place now. */
  holding: number
  /** The waiting transfers, in line. */
  places: PlaceView[]
  /** What the user should know about the saved list, if anything. */
  notice: string | null
  /** When the data used was last zeroed, in Unix seconds. */
  used_since: number
}

/**
 * What the live socket sends: the table on connect, then the downloads, the
 * torrents and the queue, then the table and its change lines on every change
 * to the links, and any of the others again on every change to them.
 */
export type LiveMessage =
  | { type: 'table'; table: TableView }
  | { type: 'change'; table: TableView; lines: string[] }
  | { type: 'downloads'; downloads: DownloadsView }
  | { type: 'torrents'; torrents: TorrentsView }
  | { type: 'queue'; queue: QueueView }
