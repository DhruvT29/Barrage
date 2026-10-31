// The words the cables column and the radar show when there are no cables to
// draw, the same in both, as the page always said them.
import type { TableView } from '../api/types'
import type { Status } from '../api/useLive'

export const NOT_ANSWERING = 'The engine is not answering. This page connects by itself once barrage runs.'
export const NO_CABLES = 'No internet connections. Join a Wi-Fi network, or plug in a phone with USB tethering on.'

/**
 * What to say instead of the cables: the engine never answered, or it has no
 * cables. Null while there are cables to show, and while still connecting
 * for the first time (nothing is known yet, so nothing is claimed).
 */
export function emptyMessage(status: Status, table: TableView | null): string | null {
  if (table === null) return status === 'disconnected' ? NOT_ANSWERING : null
  return table.links.length === 0 ? NO_CABLES : null
}

/** "03": a count in the design's two digits; "-" before the engine has said. */
export function count(n: number | null): string {
  return n === null ? '-' : String(n).padStart(2, '0')
}
