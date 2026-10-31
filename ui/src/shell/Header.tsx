import { useRef, type KeyboardEvent, type ReactNode } from 'react'

import type { Status } from '../api/useLive'
import type { PageProps } from '../links/Dashboard'
import { heroFigure, linkedCables, mbpsFigure } from '../overview/figures'
import { TABS, type Tab } from '../tabs'
import { DownloadIcon, PulseIcon, RadarIcon } from './icons'
import './shell.css'

/** The page's own connection to the engine, on screen and as said aloud. */
const SYSTEM: Record<Status, { word: string; spoken: string }> = {
  live: { word: 'Live', spoken: 'live' },
  connecting: { word: 'Connecting', spoken: 'connecting' },
  disconnected: { word: 'Offline', spoken: 'disconnected, retrying' },
}

const TAB_ICONS: Record<Tab, () => ReactNode> = { downloads: DownloadIcon, links: RadarIcon }

/**
 * The bar across the top of both tabs: the name, the two tabs, and three
 * figures: the speed now (the downloads' while anything downloads, else
 * what the cables could give together), the cables that carry data, and
 * the page's own connection to the engine, always as a word.
 */
export function Header({ status, table, downloads, tab, onTab }: PageProps & { tab: Tab; onTab: (tab: Tab) => void }) {
  const tabs = useRef<(HTMLButtonElement | null)[]>([])
  const figure = heroFigure(table, downloads)
  const linked = linkedCables(table)
  const system = SYSTEM[status]

  // Arrow keys move along the tabs and show the one they land on, as a
  // tablist does; Home and End jump to either end.
  function onKey(event: KeyboardEvent<HTMLButtonElement>, index: number) {
    const last = TABS.length - 1
    const to =
      event.key === 'ArrowRight' ? (index === last ? 0 : index + 1)
      : event.key === 'ArrowLeft' ? (index === 0 ? last : index - 1)
      : event.key === 'Home' ? 0
      : event.key === 'End' ? last
      : null
    if (to === null) return
    event.preventDefault()
    onTab(TABS[to].tab)
    tabs.current[to]?.focus()
  }

  return (
    <header className="sh-header">
      <div className="sh-inner">
        <div className="sh-brand">
          <span className="sh-mark" aria-hidden="true">
            <span />
          </span>
          <div>
            <h1 className="sh-name">BARRAGE</h1>
            <p className="sh-sub">
              Mesh <span aria-hidden="true">·</span> Bandwidth <span aria-hidden="true">·</span> Engine
            </p>
          </div>
        </div>

        <div className="sh-tabs" role="tablist" aria-label="Views">
          {TABS.map(({ tab: t, label }, i) => {
            const TabIcon = TAB_ICONS[t]
            const selected = t === tab
            return (
              <button
                key={t}
                ref={(el) => {
                  tabs.current[i] = el
                }}
                type="button"
                role="tab"
                className="sh-tab"
                aria-selected={selected}
                tabIndex={selected ? 0 : -1}
                onClick={() => onTab(t)}
                onKeyDown={(event) => onKey(event, i)}
              >
                <TabIcon />
                <span>{label}</span>
              </button>
            )
          })}
        </div>

        <div className="sh-figures">
          <div className="sh-fig" title={figure.mode === 'live' ? 'What every download fetches right now' : 'The combined speed the cables can give'}>
            <span className="sh-label">Total throughput</span>
            <span className="sh-value" data-testid="throughput">
              {figure.bps === null ? '-' : <>{mbpsFigure(figure.bps)}<small>Mbps</small></>}
            </span>
          </div>
          <div className="sh-fig" title="Cables up or degraded: the ones that carry data">
            <span className="sh-label">Links</span>
            <span className="sh-value" data-testid="linked">
              {linked === null ? '-' : <>{String(linked).padStart(2, '0')}<small>Linked</small></>}
            </span>
          </div>
          <div
            className={`sh-fig sh-system ${status}`}
            role="status"
            data-testid="light"
            aria-label={`System: ${system.spoken}`}
            title={`The page's connection to the engine: ${system.spoken}`}
          >
            <span className="sh-label">System</span>
            <span className="sh-value">
              <span className="sh-word">{system.word}</span>
              <PulseIcon />
            </span>
          </div>
        </div>
      </div>
    </header>
  )
}
