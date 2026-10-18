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

/** What the live socket sends: the table on connect, then the table and its change lines on every change. */
export type LiveMessage =
  | { type: 'table'; table: TableView }
  | { type: 'change'; table: TableView; lines: string[] }
