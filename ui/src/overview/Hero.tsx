import { useRef, useState, type CSSProperties, type ReactNode } from 'react'

import { formatBytes, percent } from '../downloads/format'
import type { PageProps } from '../links/Dashboard'
import { mbps } from '../links/cables'
import {
  ActiveIcon,
  ArrowIcon,
  FetchedIcon,
  NodesIcon,
  PingIcon,
  PlusIcon,
  RadarIcon,
  StopIcon,
} from '../shell/icons'
import type { Tab } from '../tabs'
import { stopDownload } from './control'
import {
  LIVE_NOTE,
  averagePing,
  clock,
  countedNote,
  downloadingCount,
  eta,
  fetched,
  gaugeCaption,
  heroFigure,
  leadTransfer,
  linkedCables,
  mbpsFigure,
  notCountedText,
  refusalLines,
  stoppable,
  type HeroFigure,
} from './figures'
import { Gauge } from './Gauge'
import { useConnectedFor } from './session'
import { SpeedChart, type ChartSeries } from './SpeedChart'
import './hero.css'

/** A cable's colour, or the dim one for a cable gone from the table. */
function colorOf(colors: Record<string, string>, name: string): string {
  return colors[name] ?? 'var(--dim)'
}

/**
 * The page's main panel, at the top of the download manager. The dial is
 * the live download speed while anything downloads, else the combined speed
 * available, split by cable; beside it the same figure as an equation of
 * the cables, the last minute as a chart, and on the right the actions and
 * the page's key counts. Every figure is the engine's; unknown is "-".
 */
export function Hero({
  status,
  table,
  downloads,
  history,
  colors,
  onAdd,
  onTab,
}: PageProps & { onAdd: () => void; onTab: (tab: Tab) => void }) {
  const figure = heroFigure(table, downloads)
  const live = figure.mode === 'live'
  const connectedFor = useConnectedFor(table !== null)
  const regionName = live ? 'Downloading at' : table ? 'Combined speed available' : 'Overview'

  return (
    <section className="hero" aria-label={regionName}>
      <span className="hero-corner tl" aria-hidden="true" />
      <span className="hero-corner br" aria-hidden="true" />

      <div className="hero-dial-col">
        <Dial figure={figure} colors={colors} />
      </div>

      <div className="hero-main">
        <div className="hero-meta">
          <Badge status={status} live={live} />
          <span className="hero-connected" data-testid="connected">
            {status === 'disconnected' ? (
              <>Retrying</>
            ) : connectedFor === null ? (
              <>Waiting for the engine</>
            ) : (
              <>
                Connected <b>{clock(connectedFor)}</b>
              </>
            )}
          </span>
        </div>

        <h2 className="hero-title">
          One pipe. <em>Every link.</em>
        </h2>
        <p className="hero-lede">{lede(status, table, live)}</p>

        <Equation figure={figure} colors={colors} />
        <Stack figure={figure} colors={colors} />

        <SpeedChart
          label={live ? 'Download speed' : 'Measured speed'}
          samples={history.liveTotal.length}
          zeroUnknown={!live}
          quiet={
            live ? 'Nothing arrived in the last minute'
            : table ? 'Nothing measured in the last minute'
            : 'Nothing from the engine yet'
          }
          series={chartSeries(
            live ? history.live : history.measured,
            figure.parts.map((p) => p.name),
            colors,
            live ? [] : countedOnce(table),
          )}
        />

        {(live || table) && (
          <p className="hero-note" data-testid="note">
            {live ? LIVE_NOTE : countedNote(table!.combined)}
          </p>
        )}
      </div>

      <Side {...{ table, downloads, onAdd, onTab }} />
    </section>
  )
}

/** The two lines under the headline: what BARRAGE does, or why there is nothing to show. */
function lede(status: PageProps['status'], table: PageProps['table'], live: boolean): string {
  if (!table) {
    return status === 'disconnected'
      ? 'No figures until the engine answers. This page keeps trying by itself, and connects once barrage runs.'
      : "Waiting for the engine's first table of connections."
  }
  if (table.links.length === 0) {
    return 'Nothing to split across yet: no connection is up. Each one that comes up joins in by itself.'
  }
  return (
    'Each download is split across every working connection at once, in proportion to what each delivers. ' +
    (live ? 'The dial is their sum right now.' : 'The dial is what they can give together.')
  )
}

function Badge({ status, live }: { status: PageProps['status']; live: boolean }) {
  const [word, kind] =
    status === 'disconnected' ? ['Offline', 'off']
    : status === 'connecting' ? ['Connecting', 'wait']
    : live ? ['Aggregate active', 'on']
    : ['Idle', 'idle']
  return (
    <span className={`hero-badge ${kind}`}>
      <i aria-hidden="true" />
      <span data-testid="badge">{word}</span>
    </span>
  )
}

/** The dial: filled to the share of the available speed in use while downloading, whole otherwise. */
function Dial({ figure, colors }: { figure: HeroFigure; colors: Record<string, string> }) {
  const { bps, parts, available } = figure
  const sum = parts.reduce((a, p) => a + p.bps, 0)
  const filled =
    bps === null || bps <= 0 ? 0
    : figure.mode === 'live' && available ? Math.min(1, bps / available)
    : 1
  const [value, unit] = bps === null ? ['-', null] : mbps(bps).split(' ')
  return (
    <Gauge
      filled={filled}
      parts={sum > 0 ? parts.map((p) => ({ name: p.name, share: p.bps / sum, color: colorOf(colors, p.name) })) : []}
      figure={value}
      unit={unit}
      caption={gaugeCaption(figure)}
    />
  )
}

/** One chip per part with its figure, "+" between, "=" and the sum. */
function Equation({ figure, colors }: { figure: HeroFigure; colors: Record<string, string> }) {
  if (figure.parts.length === 0 || figure.bps === null) return null
  return (
    <div className="hero-equation">
      {figure.parts.map((p, i) => (
        <span key={p.name} className="hero-eq-item">
          {i > 0 && (
            <span className="hero-op" aria-hidden="true">
              +
            </span>
          )}
          <span
            className="hero-chip"
            data-testid="part"
            title={`${p.name}: ${mbps(p.bps)}`}
            style={{ '--c': colorOf(colors, p.name) } as CSSProperties}
          >
            <i aria-hidden="true" />
            <b>{p.name}</b> {mbpsFigure(p.bps)}
          </span>
        </span>
      ))}
      <span className="hero-eq-item">
        <span className="hero-op" aria-hidden="true">
          =
        </span>
        <span className="hero-chip sum" data-testid="sum">
          {mbps(figure.bps)}
        </span>
      </span>
    </div>
  )
}

/** The same split as one thin bar. */
function Stack({ figure, colors }: { figure: HeroFigure; colors: Record<string, string> }) {
  const { bps, parts } = figure
  return (
    <div className="hero-stack" aria-hidden="true">
      {bps !== null &&
        bps > 0 &&
        parts.map((p) => (
          <span
            key={p.name}
            data-testid="segment"
            style={{ width: `${(p.bps / bps) * 100}%`, background: colorOf(colors, p.name), color: colorOf(colors, p.name) }}
          />
        ))}
    </div>
  )
}

/**
 * The cables the combined speed counts once with another, being on the same
 * connection: stacking their measured speed too would count it twice.
 */
function countedOnce(table: PageProps['table']): string[] {
  return table?.combined.parts.flatMap((p) => p.shared_with) ?? []
}

/**
 * The cables' series, stacked in the order of the chips (the figure's
 * parts), then the rest in table order, then any gone from the table; a
 * cable with nothing in the minute, or one in `leave`, is left out.
 */
function chartSeries(
  byCable: Record<string, number[]>,
  first: string[],
  colors: Record<string, string>,
  leave: string[],
): ChartSeries[] {
  const names = [...new Set([...first, ...Object.keys(colors), ...Object.keys(byCable)])]
  return names
    .filter((name) => !leave.includes(name) && byCable[name]?.some((v) => v > 0))
    .map((name) => ({ name, color: colorOf(colors, name), values: byCable[name] }))
}

/** The right column: the actions, four counts, the lead transfer and what the sum left out. */
function Side({
  table,
  downloads,
  onAdd,
  onTab,
}: Pick<PageProps, 'table' | 'downloads'> & { onAdd: () => void; onTab: (tab: Tab) => void }) {
  const ping = averagePing(table)
  const linked = linkedCables(table)
  const bytes = fetched(downloads)
  const lead = leadTransfer(downloads)
  const leftOut = table?.combined.not_counted ?? []

  return (
    <div className="hero-side">
      <div className="hero-actions">
        <button type="button" className="hero-btn primary" onClick={onAdd}>
          <PlusIcon />
          Add download
        </button>
        <StopAll downloads={downloads} />
      </div>

      <div className="hero-tiles">
        <Tile label="Links" icon={<NodesIcon />} value={linked === null ? '-' : String(linked)} />
        <Tile label="Avg ping" icon={<PingIcon />} value={ping === null ? '-' : `${ping} ms`} />
        <Tile label="Active" icon={<ActiveIcon />} value={downloads ? String(downloadingCount(downloads)) : '-'} />
        <Tile label="Fetched" icon={<FetchedIcon />} value={bytes === null ? '-' : formatBytes(bytes)} />
      </div>

      {lead && <Lead download={lead} />}

      {leftOut.length > 0 && (
        <div className="hero-notice" data-testid="not-counted">
          <RadarIcon />
          <p>{notCountedText(leftOut)}</p>
          <button type="button" className="hero-notice-go" aria-label="Open the device radar" onClick={() => onTab('links')}>
            <ArrowIcon />
          </button>
        </div>
      )}
    </div>
  )
}

function Tile({ label, icon, value }: { label: string; icon: ReactNode; value: string }) {
  return (
    <div className="hero-tile" role="group" aria-label={label}>
      <span className="hero-tile-label">
        {icon}
        {label}
      </span>
      <span className={value === '-' ? 'hero-tile-value unknown' : 'hero-tile-value'}>{value}</span>
    </div>
  )
}

/** The download with the most left to fetch: its name, how far along, and the time left at its speed. */
function Lead({ download: d }: { download: NonNullable<ReturnType<typeof leadTransfer>> }) {
  const pct = d.length === null ? null : percent(d.done, d.length)
  return (
    <div className="hero-lead" role="group" aria-label="Lead transfer">
      <span className="hero-tile-label">Lead transfer</span>
      <span className="hero-lead-name" title={d.name ?? d.url}>
        {d.name ?? d.url}
      </span>
      <div
        className="hero-lead-bar"
        role="progressbar"
        aria-label={`${d.name ?? d.url} progress`}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={pct ?? undefined}
      >
        <span style={{ width: `${pct ?? 0}%` }} />
      </div>
      <div className="hero-lead-foot">
        <span>{pct === null ? `${formatBytes(d.done)} so far` : `${pct}%`}</span>
        <span>ETA {eta(d)}</span>
      </div>
    </div>
  )
}

/**
 * Stops every download a card's Stop would, all at once, while any runs.
 * Waits for every answer before it can be pressed again; a refusal is
 * written under it in the engine's words, by download.
 */
function StopAll({ downloads }: { downloads: PageProps['downloads'] }) {
  const running = stoppable(downloads)
  const [sending, setSending] = useState(false)
  const [refused, setRefused] = useState<string[]>([])
  // A second press before the page redraws must not ask twice.
  const busy = useRef(false)

  if (running.length === 0) return null

  async function stopAll() {
    if (busy.current) return
    busy.current = true
    setSending(true)
    setRefused([])
    const answers = await Promise.all(running.map(async (d) => ({ d, answer: await stopDownload(d.id) })))
    busy.current = false
    setSending(false)
    const words = answers.flatMap(({ d, answer }) => ('refused' in answer ? [{ name: d.name ?? d.url, words: answer.refused }] : []))
    setRefused(refusalLines(words, answers.length))
  }

  return (
    <>
      <button type="button" className="hero-btn ghost" onClick={stopAll} disabled={sending}>
        <StopIcon />
        Stop all
      </button>
      {refused.length > 0 && (
        <div className="hero-refused" role="alert">
          {refused.map((line) => (
            <p key={line}>{line}</p>
          ))}
        </div>
      )}
    </>
  )
}
