import type { TableView } from '../api/types'
import { joinNames } from './cables'

/**
 * One notice per group of cables that reach the internet through one
 * connection, in `brg-links`'s own words (render.rs's footer). A group whose
 * public address is not known is left out, as there.
 */
export function SharedNotice({ table }: { table: TableView }) {
  const notices = table.shared.flatMap((group) => {
    const members = group.flatMap((id) => table.links.filter((l) => l.id === id))
    const publicIp = members[0]?.public_ip
    if (!publicIp) return []
    const names = [...new Set(members.map((l) => l.name))]
    return [{ key: group.join('-'), names: joinNames(names), publicIp }]
  })

  return notices.map((n) => (
    <div className="ml-notice" key={n.key} data-testid="shared-notice">
      <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round">
        <circle cx="8" cy="8" r="6.5" />
        <path d="M8 4.5v4.2M8 11.2v.1" />
      </svg>
      <p>
        <strong>One connection.</strong> {n.names} reach the internet through one connection ({n.publicIp}), so
        using both gains nothing.
      </p>
    </div>
  ))
}
