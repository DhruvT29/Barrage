import { useEffect, useId, useRef, useState, type CSSProperties, type ReactNode, type RefObject } from 'react'

import type { CableShare, DownloadState, DownloadView } from '../api/types'
import { mbps } from '../links/cables'
import { controlDownload } from '../overview/control'
import { formatBytes, percent, percentText } from './format'
import {
  CableIcon,
  ChevronIcon,
  ClockIcon,
  ConnectionsIcon,
  DriveIcon,
  GaugeIcon,
  PauseIcon,
  PlayIcon,
} from './icons'
import { blockCount, etaText, pieceBlocks } from './queue'

import './downloads.css'

/** What a download in this state can be asked: Stop while it runs, Resume once stopped. */
function actionFor(state: DownloadState): 'stop' | 'resume' | null {
  if (state === 'starting' || state === 'waiting' || state === 'downloading') return 'stop'
  return state === 'stopped' ? 'resume' : null
}

/**
 * The card's button: what it asks, whether an answer is awaited, and the
 * engine's last refusal, shown only while the download is in the state it
 * was refused in. The card itself changes when the engine's next status
 * says so.
 */
function useAction(id: number, state: DownloadState) {
  const action = actionFor(state)
  const [refused, setRefused] = useState<{ words: string; state: DownloadState } | null>(null)
  const [sending, setSending] = useState(false)
  // A second press before the page redraws must not ask twice.
  const busy = useRef(false)

  async function press() {
    if (action === null || busy.current) return
    busy.current = true
    setSending(true)
    setRefused(null)
    const asked = await controlDownload(id, action)
    busy.current = false
    setSending(false)
    if ('refused' in asked) setRefused({ words: asked.refused, state })
  }

  return { action, sending, refused: refused?.state === state ? refused.words : null, press }
}

/** What a card says while it has no speed to show and is still under way. */
const WAITING_WORDS: Partial<Record<DownloadState, string>> = {
  starting: 'finding the file',
  waiting: 'no cable to fetch it through yet',
}

/** The address's host, to tell where a file comes from at a glance; null if it does not parse. */
function hostOf(url: string): string | null {
  try {
    return new URL(url).host || null
  } catch {
    return null
  }
}

type CardProps = {
  download: DownloadView
  /** Each cable's colour by name (palette.ts); a cable missing from it is dim. */
  colors?: Record<string, string>
  /** Whether the list of cables is open, when the queue holds the user's choice; otherwise the card does. */
  expanded?: boolean
  /** Told the user's choice to open (true) or close the list of cables. */
  onToggle?: (open: boolean) => void
}

/**
 * One download in the transfer queue: its state in a word, its number, its
 * name (its address until the file is found) and host; the Stop or Resume
 * it can be asked; the piece bar, filled in each cable's colour by what that
 * cable carried, with the percent done; the five figures (fetched, speed,
 * time left, connections, cables); the engine's note; and, behind a toggle,
 * a row per cable. Every note is the engine's own words.
 *
 * The state is a colour (from the `tq-<state>` class) and the engine's word,
 * always written out. The list of cables starts open while downloading and
 * closed otherwise, until the user opens or closes it.
 */
export function DownloadCard({ download: d, colors = {}, expanded, onToggle }: CardProps) {
  const title = d.name ?? d.url
  const host = d.name !== null ? hostOf(d.url) : null
  const moving = d.state === 'downloading'
  const words = WAITING_WORDS[d.state]
  const { action, sending, refused, press } = useAction(d.id, d.state)
  const [mine, setMine] = useState<boolean | null>(null)
  const open = expanded ?? mine ?? moving

  // A cable's note that repeats the download's own says nothing new, and a
  // cable that carried nothing, holds nothing open, is wanted none and has
  // nothing new to say is left out.
  const rows = d.cables
    .map((c) => ({ ...c, notes: c.notes.filter((n) => n !== d.note) }))
    .filter((c) => c.bytes > 0 || c.connections > 0 || c.notes.length > 0 || (moving && c.wanted > 0))

  function toggle() {
    setMine(!open)
    onToggle?.(!open)
  }

  return (
    <section className={`tq-card tq-${d.state}`} aria-label={title}>
      <div className="tq-head">
        <div className="tq-ident">
          <div className="tq-tags">
            <span className="tq-badge">
              {moving && <span className="tq-dot" aria-hidden="true" />}
              <span data-testid="state">{d.state}</span>
            </span>
            <span className="tq-id" data-testid="id">
              #{d.id}
            </span>
          </div>
          <h3 className="tq-name">{title}</h3>
          {host !== null && (
            <span className="tq-host" data-testid="host" title={d.url}>
              {host}
            </span>
          )}
        </div>
        {action !== null && (
          <div className="tq-actions">
            <button
              type="button"
              className={`tq-act tq-${action}`}
              aria-label={action === 'stop' ? 'Stop' : 'Resume'}
              title={action === 'stop' ? 'Stop' : 'Resume'}
              disabled={sending}
              onClick={press}
            >
              {action === 'stop' ? <PauseIcon /> : <PlayIcon />}
            </button>
          </div>
        )}
      </div>

      <div className="tq-progress">
        <PieceBar download={d} colors={colors} />
        <span className={d.state !== 'done' && d.length === null ? 'tq-percent unknown' : 'tq-percent'} data-testid="percent">
          {d.state === 'done' ? '100.0%' : percentText(d.done, d.length)}
        </span>
      </div>

      <Stats download={d} />

      {words && (
        <p className="tq-words" data-testid="words">
          {words}
        </p>
      )}
      {d.note !== null && (
        <p className={d.state === 'failed' ? 'tq-note failure' : 'tq-note'} data-testid="note">
          {d.note}
        </p>
      )}
      {refused !== null && (
        <p className="tq-refused" role="alert">
          {refused}
        </p>
      )}

      <Distribution rows={rows} open={open} onToggle={toggle} download={d} colors={colors} />
    </section>
  )
}

/** As many blocks as fit the bar's width, measured as it changes; 64 until measured. */
function useBlockCount(ref: RefObject<HTMLElement | null>): number {
  const [count, setCount] = useState(64)
  useEffect(() => {
    const element = ref.current
    if (!element || typeof ResizeObserver === 'undefined') return
    const observer = new ResizeObserver(([entry]) => setCount(blockCount(entry.contentRect.width)))
    observer.observe(element)
    return () => observer.disconnect()
  }, [ref])
  return count
}

/**
 * The piece bar: a row of blocks filled up to the part done, in the colours
 * of the cables that fetched it (queue.ts), the rest dim. Without a length
 * there is nothing to fill against: while the download is under way, a
 * shimmer runs along the dim blocks instead.
 */
function PieceBar({ download: d, colors }: { download: DownloadView; colors: Record<string, string> }) {
  const ref = useRef<HTMLDivElement>(null)
  const count = useBlockCount(ref)
  const blocks = pieceBlocks(d, colors, count)
  // The filled blocks come first, so the last of them is the leading edge.
  const lastOn = (blocks ?? []).filter((b) => b !== null).length - 1
  const known = d.state === 'done' ? 100 : d.length !== null ? percent(d.done, d.length) : null

  return (
    <div
      ref={ref}
      className={blocks === null ? 'tq-pieces unknown' : 'tq-pieces'}
      role="progressbar"
      aria-label="Progress"
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={known ?? undefined}
    >
      {(blocks ?? Array<null>(count).fill(null)).map((color, i) => (
        <span
          key={i}
          data-testid="piece"
          className={color === null ? 'tq-piece' : i === lastOn ? 'tq-piece on edge' : 'tq-piece on'}
          style={color === null ? undefined : ({ '--c': color } as CSSProperties)}
        />
      ))}
    </div>
  )
}

/** Fetched of the whole: "16.6 GB / 34.2 GB"; "-" before a byte or a length. */
function fetched(d: DownloadView): ReactNode {
  const whole = d.state === 'done' ? (d.length ?? d.done) : d.length
  const got = d.state === 'done' ? whole! : d.done
  if (whole === null && got === 0) return '-'
  return (
    <>
      <b>{formatBytes(got)}</b>
      <span className="tq-of"> / {whole === null ? '-' : formatBytes(whole)}</span>
    </>
  )
}

/**
 * The five figures under the bar: what is fetched of the whole, the speed
 * (only while downloading), the time left (or the state in a word), the
 * connections open on every cable, and how many cables carried it.
 */
function Stats({ download: d }: { download: DownloadView }) {
  const connections = d.cables.reduce((sum, c) => sum + c.connections, 0)
  const carrying = d.cables.filter((c) => c.bytes > 0 || c.connections > 0).length
  const [figure, unit] = mbps(d.bps).split(' ')

  return (
    <dl className="tq-stats">
      <Stat icon={<DriveIcon />} label="Fetched" id="progress">
        {fetched(d)}
      </Stat>
      <Stat icon={<GaugeIcon />} label="Speed" id="speed" tone={d.state === 'downloading' ? 'live' : undefined}>
        {d.state === 'downloading' ? (
          <>
            {figure} <small>{unit}</small>
          </>
        ) : (
          '-'
        )}
      </Stat>
      <Stat icon={<ClockIcon />} label="ETA" id="eta">
        {etaText(d)}
      </Stat>
      <Stat icon={<ConnectionsIcon />} label="Connections" id="connections">
        {connections > 0 ? connections : '-'}
      </Stat>
      <Stat icon={<CableIcon />} label="Cables" id="cables">
        {carrying}
      </Stat>
    </dl>
  )
}

function Stat({
  icon,
  label,
  id,
  tone,
  children,
}: {
  icon: ReactNode
  label: string
  id: string
  tone?: 'live'
  children: ReactNode
}) {
  return (
    <div className={tone === 'live' ? 'tq-stat live' : 'tq-stat'}>
      <dt>
        {icon}
        {label}
      </dt>
      <dd data-testid={id}>{children}</dd>
    </div>
  )
}

/**
 * The card's foot: "Link distribution · 2 cables", a toggle for the list of
 * cables below it. With no cable to list there is nothing to open, so it is
 * a plain line, not a button.
 */
function Distribution({
  rows,
  open,
  onToggle,
  download: d,
  colors,
}: {
  rows: CableShare[]
  open: boolean
  onToggle: () => void
  download: DownloadView
  colors: Record<string, string>
}) {
  const listId = useId()
  const moving = d.state === 'downloading'

  if (rows.length === 0) {
    return (
      <div className="tq-dist" data-testid="distribution">
        <span className="tq-dist-toggle static">Link distribution · no cables</span>
      </div>
    )
  }

  return (
    <div className={open ? 'tq-dist open' : 'tq-dist'} data-testid="distribution">
      <button type="button" className="tq-dist-toggle" aria-expanded={open} aria-controls={listId} onClick={onToggle}>
        {`Link distribution · ${rows.length} ${rows.length === 1 ? 'cable' : 'cables'}`}
        <ChevronIcon />
      </button>
      <ul id={listId} className="tq-shares" aria-label="Cables" hidden={!open}>
        {rows.map((c) => (
          <Share key={c.name} cable={c} color={colors[c.name] ?? 'var(--dim)'} done={d.done} bps={d.bps} moving={moving} />
        ))}
      </ul>
    </div>
  )
}

/**
 * One cable's part: its share of the bytes so far, and while it downloads
 * its speed, its share of the download's speed and its connections, with
 * what the download wants it to hold when that differs.
 */
function Share({
  cable: c,
  color,
  done,
  bps,
  moving,
}: {
  cable: CableShare
  color: string
  done: number
  bps: number
  moving: boolean
}) {
  const share = done > 0 ? Math.min(100, (c.bytes * 100) / done) : 0
  const out = c.bytes === 0 && c.connections === 0
  // A cable with nothing open and nothing measured has no speed to give.
  const speed = !moving ? '' : c.connections > 0 || c.bps > 0 ? mbps(c.bps) : '-'
  const taking = moving && (c.connections > 0 || c.wanted > 0)
  const carrying = taking && bps > 0 ? Math.floor((c.bps * 100) / bps) : null

  return (
    <li className={out ? 'tq-share out' : 'tq-share'} data-testid="cable" style={{ '--c': color } as CSSProperties}>
      <span className="tq-share-dot" aria-hidden="true" />
      <span className="tq-share-name" data-testid="cable-name">
        {c.name}
      </span>
      <span className="tq-share-bar" aria-hidden="true">
        <span data-testid="cable-share" style={{ width: `${share}%` }} />
      </span>
      <span className="tq-share-bytes" data-testid="cable-bytes">
        {formatBytes(c.bytes)}
      </span>
      <span className="tq-share-speed" data-testid="cable-speed">
        {speed}
      </span>
      {taking && (
        <span className="tq-share-detail" data-testid="cable-detail">
          {carrying !== null && <span data-testid="cable-carrying">carrying {carrying}%</span>}
          {carrying !== null && ' · '}
          <span data-testid="cable-connections">{`${c.connections} connection${c.connections === 1 ? '' : 's'}`}</span>
          {c.wanted !== c.connections && (
            <>
              , <b data-testid="cable-wants">wants {c.wanted}</b>
            </>
          )}
        </span>
      )}
      {c.notes.map((n) => (
        <span className="tq-share-note" data-testid="cable-note" key={n}>
          {n}
        </span>
      ))}
    </li>
  )
}
