import { useReducer, useState } from 'react'

import type { PageProps } from '../links/Dashboard'
import { DownloadCard } from './DownloadCard'
import { TrayIcon } from './icons'
import { countByFilter, FILTERS, filterOf, openChoice, rememberOpen, type Filter } from './queue'

import './downloads.css'

const FILTER_WORDS: Record<Filter, string> = {
  all: 'All',
  active: 'Active',
  stopped: 'Stopped',
  done: 'Done',
  failed: 'Failed',
}

/** What a filter with nothing in it says. */
const EMPTY_WORDS: Record<Exclude<Filter, 'all'>, string> = {
  active: 'Nothing active.',
  stopped: 'Nothing stopped.',
  done: 'Nothing done yet.',
  failed: 'Nothing failed.',
}

/**
 * The transfer queue, the download manager's right column: every download
 * since the engine started, the newest first, one card each, under a
 * heading with how many there are and a filter by state. A filter with
 * nothing in it is left out, except All, Active and the one chosen, so the
 * row never offers an empty view nor takes the chosen one away.
 *
 * Before the engine's first word on downloads it shows nothing at all,
 * rather than a moment of "nothing downloaded yet" that is not so. An empty
 * queue offers to add a download (`onAdd` opens the add window).
 */
export function TransferQueue({ downloads, colors, onAdd }: PageProps & { onAdd: () => void }) {
  const [filter, setFilter] = useState<Filter>('all')
  // The choices to open or close a card's cables live in queue.ts, for as
  // long as the page is open; this only redraws when one changes.
  const [, redraw] = useReducer((n: number) => n + 1, 0)

  const list = downloads?.list ?? []
  // The engine lists them in the order they were added.
  const newestFirst = [...list].reverse()
  const counts = countByFilter(list)
  const shown = filter === 'all' ? newestFirst : newestFirst.filter((d) => filterOf(d.state) === filter)
  const offered = FILTERS.filter((f) => f === 'all' || f === 'active' || f === filter || counts[f] > 0)

  return (
    <section className="transfer-queue tq" aria-label="Transfer queue">
      <div className="tq-top">
        <div>
          <p className="tq-eyebrow">Transfer queue</p>
          <h2 className="tq-title">
            Queue{' '}
            <span className="tq-count" data-testid="count">
              {downloads === null ? '-' : String(list.length).padStart(2, '0')}
            </span>
          </h2>
        </div>
        {downloads !== null && (
          <div className="tq-filters" role="group" aria-label="Show">
            {offered.map((f) => (
              <button
                key={f}
                type="button"
                className="tq-filter"
                aria-pressed={filter === f}
                onClick={() => setFilter(f)}
              >
                {FILTER_WORDS[f]} <span className="tq-filter-count">{counts[f]}</span>
              </button>
            ))}
          </div>
        )}
      </div>

      {downloads !== null && list.length === 0 && (
        <div className="tq-empty">
          <span className="tq-empty-mark">
            <TrayIcon />
          </span>
          <p className="tq-empty-title">Nothing downloaded yet.</p>
          <p className="tq-empty-hint">Add a link to a file to fetch it through every cable at once.</p>
          <button type="button" className="tq-add" onClick={onAdd}>
            + Add download
          </button>
        </div>
      )}
      {list.length > 0 && shown.length === 0 && filter !== 'all' && (
        <p className="tq-empty-filter">{EMPTY_WORDS[filter]}</p>
      )}

      <div className="tq-list">
        {shown.map((d) => (
          <DownloadCard
            key={d.id}
            download={d}
            colors={colors}
            expanded={openChoice(d.id)}
            onToggle={(open) => {
              rememberOpen(d.id, open)
              redraw()
            }}
          />
        ))}
      </div>
    </section>
  )
}
