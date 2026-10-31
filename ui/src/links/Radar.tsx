import './radar.css'

import { useState } from 'react'

import { ActivityFeed } from './ActivityFeed'
import { cables, liveBps, mbps } from './cables'
import type { PageProps } from './Dashboard'
import { emptyMessage } from './messages'
import { FILTER_EMPTY, FILTERS, type Filter, filterCables, RANGE_MS, stateCounts, TONE_COLOR, tone } from './radarModel'
import { RadarDetail } from './RadarDetail'
import { RadarScope, SWEEP_MS } from './RadarScope'
import { Roster } from './Roster'

/** The legend: what each colour and mark on the radar means, in words. */
const LEGEND = [
  { mark: 'rd-lg-up', text: 'Up' },
  { mark: 'rd-lg-warn', text: 'Degraded · probing · portal' },
  { mark: 'rd-lg-down', text: 'Down' },
  { mark: 'rd-lg-beam', text: 'Carrying a download' },
  { mark: 'rd-lg-hollow', text: 'No round trip yet' },
]

/**
 * The "Device radar" tab: every cable the engine watches on a radar (distance
 * by round trip, colour by state), a filter by whether it carries a download,
 * the selected cable inspected at the right, a roster of the cables and the
 * engine's change lines under it.
 *
 * The only state of its own is the filter and the selected cable's name: the
 * selection is kept by name as the table changes and falls back to the first
 * cable shown when its cable is gone or filtered out.
 */
export function RadarView({ status, table, feed, downloads, colors }: PageProps) {
  const [filter, setFilter] = useState<Filter>('all')
  const [chosen, setChosen] = useState<string | null>(null)

  const all = table ? cables(table, downloads?.used) : []
  const list = filterCables(all, filter, downloads)
  const selected = list.find((c) => c.name === chosen) ?? list[0] ?? null
  const empty = emptyMessage(status, table) ?? (all.length > 0 && list.length === 0 ? FILTER_EMPTY[filter] : null)

  const liveTotal = downloads?.live.bps ?? 0
  const usedTotal = all.reduce((sum, c) => sum + c.used, 0)
  const aggregate =
    liveTotal > 0
      ? `${mbps(liveTotal)} live`
      : table && table.combined.bps > 0
        ? `${mbps(table.combined.bps)} available`
        : '- available'
  const watch = status === 'live' ? 'watching' : status === 'disconnected' ? 'offline' : 'connecting'

  return (
    <div className="rd-page">
      <section className="rd-panel rd-scopepanel" aria-label="Link detection">
        <div className="rd-scopehead">
          <span className={`rd-badge rd-watch-${status}`} data-testid="watch">
            {watch}
          </span>
          {/* The page's one h1 is its name, in the header; this leads the tab. */}
          <h2 className="rd-h1">
            Link <span className="rd-accent">Detection</span>
          </h2>
          <p className="rd-desc">Every interface the engine watches. Select one to inspect it.</p>
          <div className="rd-filter" role="group" aria-label="Show">
            {FILTERS.map((f) => (
              <button key={f.value} type="button" aria-pressed={filter === f.value} onClick={() => setFilter(f.value)}>
                {f.label}
              </button>
            ))}
          </div>
        </div>

        <div className="rd-scopebody">
          <RadarScope
            list={list}
            downloads={downloads}
            selected={selected?.name ?? null}
            onSelect={setChosen}
            empty={empty}
            known={table !== null}
          />

          <div className="rd-side">
            <div>
              <p className="rd-eyebrow">States</p>
              <table className="rd-states" aria-label="States">
                <tbody>
                  {stateCounts(all).map(([state, n]) => (
                    <tr key={state} className={n && table ? undefined : 'rd-zero'}>
                      <th scope="row">
                        <span className="rd-pip" style={{ background: TONE_COLOR[tone(state)] }} aria-hidden="true" />
                        {state}
                      </th>
                      {/* Unknown before the first table: "-", never 0. */}
                      <td>{table ? n : '-'}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
            <div>
              <p className="rd-eyebrow">Legend</p>
              <ul className="rd-legend">
                {LEGEND.map((l) => (
                  <li key={l.text}>
                    <span className={`rd-lg ${l.mark}`} aria-hidden="true" />
                    {l.text}
                  </li>
                ))}
              </ul>
            </div>
          </div>
        </div>

        <footer className="rd-scopefoot">
          <span>
            Sweep {SWEEP_MS / 1000}s · Range {RANGE_MS} ms
          </span>
          <span>{aggregate}</span>
        </footer>
      </section>

      <RadarDetail
        cable={selected}
        color={selected ? (colors[selected.name] ?? 'var(--dim)') : 'var(--dim)'}
        liveBps={selected ? liveBps(downloads, selected.name) : null}
        liveTotal={liveTotal}
        usedTotal={usedTotal}
        downloadsKnown={downloads !== null}
        empty={empty}
      />

      <Roster
        list={list}
        all={all}
        downloads={downloads}
        selected={selected?.name ?? null}
        onSelect={setChosen}
        empty={empty}
        known={table !== null}
      />

      <ActivityFeed feed={feed} />
    </div>
  )
}
