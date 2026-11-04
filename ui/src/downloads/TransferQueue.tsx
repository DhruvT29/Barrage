import { useReducer, useState } from 'react'

import type { PageProps } from '../links/Dashboard'
import { DownloadCard } from './DownloadCard'
import { TrayIcon } from './icons'
import { countByFilter, FILTERS, filterOf, openChoice, rememberOpen, torrentFilterOf, type Filter } from './queue'
import { SwarmLine, TorrentCard } from './TorrentCard'

import './downloads.css'

const FILTER_WORDS: Record<Filter, string> = {
  all: 'All',
  active: 'Active',
  torrents: 'Torrents',
  stopped: 'Stopped',
  done: 'Done',
  failed: 'Failed',
}

/** What a filter with nothing in it says. */
const EMPTY_WORDS: Record<Exclude<Filter, 'all'>, string> = {
  active: 'Nothing active.',
  torrents: 'No torrents.',
  stopped: 'Nothing stopped.',
  done: 'Nothing done yet.',
  failed: 'Nothing failed.',
}

/**
 * The transfer queue, the download manager's right column: every torrent
 * and download since the engine started, the newest first, torrents above
 * downloads, one card each, under a heading with how many there are and a
 * filter by state, with the line on DHT and the port once there is a
 * torrent. A filter with
 * nothing in it is left out, except All, Active and the one chosen, so the
 * row never offers an empty view nor takes the chosen one away.
 *
 * Before the engine's first word on downloads it shows nothing at all,
 * rather than a moment of "nothing downloaded yet" that is not so. An empty
 * queue offers to add a download (`onAdd` opens the add window).
 */
export function TransferQueue({ downloads, torrents, colors, onAdd }: PageProps & { onAdd: () => void }) {
  const [filter, setFilter] = useState<Filter>('all')
  // The choices to open or close a card's cables live in queue.ts, for as
  // long as the page is open; this only redraws when one changes.
  const [, redraw] = useReducer((n: number) => n + 1, 0)

  const list = downloads?.list ?? []
  // The engine lists them in the order they were added.
  const newestFirst = [...list].reverse()
  const swarm = torrents?.list ?? []
  const counts = countByFilter(list, swarm)
  const shown = filter === 'all' ? newestFirst : newestFirst.filter((d) => filterOf(d.state) === filter)
  const shownTorrents = [...swarm]
    .reverse()
    .filter((t) => filter === 'all' || filter === 'torrents' || torrentFilterOf(t.phase) === filter)
  const offered = FILTERS.filter((f) => f === 'all' || f === 'active' || f === filter || counts[f] > 0)

  return (
    <section className="transfer-queue tq" aria-label="Transfer queue">
      <div className="tq-top">
        <div>
          <p className="tq-eyebrow">Transfer queue</p>
          <h2 className="tq-title">
            Queue{' '}
            <span className="tq-count" data-testid="count">
              {downloads === null ? '-' : String(counts.all).padStart(2, '0')}
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

      {torrents && swarm.length > 0 && <SwarmLine torrents={torrents} />}

      {downloads !== null && counts.all === 0 && (
        <div className="tq-empty">
          <span className="tq-empty-mark">
            <TrayIcon />
          </span>
          <p className="tq-empty-title">Nothing downloaded yet.</p>
          <p className="tq-empty-hint">Add a link to a file or a magnet link to fetch it through every cable at once.</p>
          <button type="button" className="tq-add" onClick={onAdd}>
            + Add download
          </button>
        </div>
      )}
      {counts.all > 0 && shown.length + shownTorrents.length === 0 && filter !== 'all' && (
        <p className="tq-empty-filter">{EMPTY_WORDS[filter]}</p>
      )}

      <div className="tq-list">
        {shownTorrents.map((t) => (
          <TorrentCard
            key={`t${t.id}`}
            torrent={t}
            colors={colors}
            expanded={openChoice(`t${t.id}`)}
            onToggle={(open) => {
              rememberOpen(`t${t.id}`, open)
              redraw()
            }}
          />
        ))}
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
