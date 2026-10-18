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

/** A download's state words. */
export type DownloadState = 'starting' | 'waiting' | 'downloading' | 'done' | 'failed'

/** Every download since the engine started, and the speed they make together. */
export interface DownloadsView {
  live: LiveSpeed
  /** In the order they were added. */
  list: DownloadView[]
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
}

/** One cable's part in a download: its addresses' shares added up by the engine. */
export interface CableShare {
  name: string
  bytes: number
  /** In bits per second, 0 unless downloading. */
  bps: number
  connections: number
  /** Each of its addresses' notes, each said once. */
  notes: string[]
}

/**
 * What the live socket sends: the table on connect, then the downloads, then
 * the table and its change lines on every change to the links, and the
 * downloads again on every change to them.
 */
export type LiveMessage =
  | { type: 'table'; table: TableView }
  | { type: 'change'; table: TableView; lines: string[] }
  | { type: 'downloads'; downloads: DownloadsView }
