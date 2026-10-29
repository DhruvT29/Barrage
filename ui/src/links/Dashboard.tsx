import type { FeedLine, Status } from '../api/useLive'
import type { DownloadsView, TableView } from '../api/types'
import { Downloads } from '../downloads/Downloads'
import { TABS, type Tab } from '../tabs'
import { ActivityFeed } from './ActivityFeed'
import { cables } from './cables'
import { Combined } from './Combined'
import { ConnectionLight } from './ConnectionLight'
import { LinkCard } from './LinkCard'
import { SharedNotice } from './SharedNotice'

/**
 * The whole page, from what `useLive` holds and the tab shown: the header
 * bar with the tabs, the main figure across the full width, then the tab's
 * view beside the activity column. Pure: the same props always draw the same
 * page, and a tab clicked only asks for itself through `onTab`.
 */
export function Dashboard({
  status,
  table,
  feed,
  downloads,
  tab,
  onTab,
}: {
  status: Status
  table: TableView | null
  feed: FeedLine[]
  downloads: DownloadsView | null
  tab: Tab
  onTab: (tab: Tab) => void
}) {
  // Disconnected with a table: the last one stays, dimmed, so it reads as old.
  const stale = status === 'disconnected' && table !== null

  return (
    <>
      <header className="bar">
        <h1 className="wordmark">BARRAGE</h1>
        <nav className="tabs" role="tablist" aria-label="Views">
          {TABS.map(({ tab: id, label }) => (
            <button key={id} type="button" role="tab" aria-selected={tab === id} onClick={() => onTab(id)}>
              {label}
            </button>
          ))}
        </nav>
        <ConnectionLight status={status} />
      </header>

      <div className={stale ? 'layout stale' : 'layout'}>
        {table && <Combined combined={table.combined} downloads={downloads} />}
        {tab === 'downloads' ? (
          <Downloads downloads={downloads} />
        ) : (
          <main className="links">{table ? <Links table={table} /> : <Waiting status={status} />}</main>
        )}
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
