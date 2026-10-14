// The engine's JSON, field for field, as `engine/src/api/view.rs` writes it.
// That file is the contract: change both together.

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
}

/** What the live socket sends: the table on connect, then the table and its change lines on every change. */
export type LiveMessage =
  | { type: 'table'; table: TableView }
  | { type: 'change'; table: TableView; lines: string[] }
