import type { FeedLine, Status } from '../api/useLive'
import type { TableView } from '../api/types'
import { ActivityFeed } from './ActivityFeed'
import { cables } from './cables'
import { Combined } from './Combined'
import { ConnectionLight } from './ConnectionLight'
import { LinkCard } from './LinkCard'
import { SharedNotice } from './SharedNotice'

/**
 * The whole page, from what `useLive` holds: the header bar, the combined
 * speed across the full width, then the cards beside the activity column.
 * Pure: the same props always draw the same page.
 */
export function Dashboard({ status, table, feed }: { status: Status; table: TableView | null; feed: FeedLine[] }) {
  // Disconnected with a table: the last one stays, dimmed, so it reads as old.
  const stale = status === 'disconnected' && table !== null

  return (
    <>
      <header className="bar">
        <h1 className="wordmark">BARRAGE</h1>
        <nav className="tabs" role="tablist" aria-label="Views">
          <button type="button" role="tab" aria-selected="true">
            Links
          </button>
        </nav>
        <ConnectionLight status={status} />
      </header>

      <div className={stale ? 'layout stale' : 'layout'}>
        {table && <Combined combined={table.combined} />}
        <main className="links">{table ? <Links table={table} /> : <Waiting status={status} />}</main>
        <ActivityFeed feed={feed} />
      </div>
    </>
  )
}

function Links({ table }: { table: TableView }) {
  const all = cables(table)
  if (all.length === 0) {
    return (
      <p className="quiet wide">
        No internet connections. Join a Wi-Fi network, or plug in a phone with USB tethering on.
      </p>
    )
  }
  // One scale for every card: the fastest cable fills its own beam.
  const speeds = all.flatMap((c) => (c.downBps === null ? [] : [c.downBps]))
  const fastestBps = speeds.length > 0 ? Math.max(...speeds) : null
  return (
    <>
      <SharedNotice table={table} />
      {all.map((cable) => (
        <LinkCard key={cable.name} cable={cable} fastestBps={fastestBps} />
      ))}
    </>
  )
}

/** Before any table: say nothing while connecting, say why once the engine fails to answer. */
function Waiting({ status }: { status: Status }) {
  if (status !== 'disconnected') return null
  return (
    <p className="quiet wide">The engine is not answering. This page connects by itself once barrage runs.</p>
  )
}
