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
    <div className="panel shared" key={n.key} data-testid="shared-notice">
      <p>
        <strong>One connection.</strong> {n.names} reach the internet through one connection ({n.publicIp}), so
        using both gains nothing.
      </p>
    </div>
  ))
}
