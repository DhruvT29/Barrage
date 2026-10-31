import './links.css'

import { cables, fastest, liveBps } from './cables'
import type { PageProps } from './Dashboard'
import { LinkCard } from './LinkCard'
import { count, emptyMessage } from './messages'
import { SharedNotice } from './SharedNotice'

/**
 * The download manager's left column: every cable the engine has, a card
 * each in the table's order, with the shared-connection notice above them
 * when cables share one. Says why when there is nothing to show.
 */
export function MeshLinks({ status, table, downloads, history, colors }: PageProps) {
  const list = table ? cables(table, downloads?.used) : []
  const best = fastest(list)
  const message = emptyMessage(status, table)
  const liveTotal = downloads?.live.bps ?? 0

  return (
    <section className="ml-column" aria-label="Mesh links">
      <div className="ml-columnhead">
        <p className="ml-eyebrow">Plugged interfaces</p>
        <h2 className="ml-title">
          Mesh links <span className="ml-count">{count(table ? list.length : null)}</span>
        </h2>
      </div>

      {table && <SharedNotice table={table} />}
      {message && <p className="ml-empty">{message}</p>}

      <div className="ml-cards">
        {list.map((cable) => {
          const live = liveBps(downloads, cable.name)
          const series = (live !== null ? history.live[cable.name] : history.measured[cable.name]) ?? []
          return (
            <LinkCard
              key={cable.name}
              cable={cable}
              color={colors[cable.name] ?? 'var(--dim)'}
              liveBps={live}
              liveTotal={liveTotal}
              fastest={cable.name === best}
              series={series}
            />
          )
        })}
      </div>
    </section>
  )
}
