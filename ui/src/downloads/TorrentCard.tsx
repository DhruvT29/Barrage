import { useId, useState, type CSSProperties } from 'react'

import type { TorrentCableView, TorrentPhase, TorrentsView, TorrentView } from '../api/types'
import { mbps } from '../links/cables'
import { controlTorrent, setPriority } from '../overview/control'
import { PieceBar, Stat } from './DownloadCard'
import { formatBytes, percentText } from './format'
import { ChevronIcon, ClockIcon, ConnectionsIcon, DriveIcon, GaugeIcon, PauseIcon, PlayIcon, UploadIcon } from './icons'
import { torrentBar, torrentEta } from './queue'
import { TransferControls } from './TransferControls'
import { useAction } from './useAction'

import './downloads.css'

/** What a torrent in this phase can be asked: Stop while it runs or waits in line, Resume once stopped. */
function actionFor(phase: TorrentPhase): 'stop' | 'resume' | null {
  if (phase === 'queued' || phase === 'finding' || phase === 'downloading') return 'stop'
  return phase === 'stopped' ? 'resume' : null
}

/** The card's colours are the download card's: finding reads as starting. */
function cardClass(phase: TorrentPhase): string {
  return `tq-card tq-torrent tq-${phase === 'finding' ? 'starting' : phase}`
}

type CardProps = {
  torrent: TorrentView
  /** Each cable's colour by name (palette.ts); a cable missing from it is dim. */
  colors?: Record<string, string>
  /** Whether the list of cables is open, when the queue holds the user's choice; otherwise the card does. */
  expanded?: boolean
  /** Told the user's choice to open (true) or close the list of cables. */
  onToggle?: (open: boolean) => void
  /** Its place in line, while it waits for one. */
  place?: number
}

/**
 * One torrent in the transfer queue, in the download card's style: its phase
 * in a word, its number, its name (its info hash until the name is known);
 * the Stop or Resume it can be asked; the piece bar, split by what each cable
 * received; the five figures (fetched, down, up, peers, time left); the
 * engine's note; and, behind a toggle, a row per cable with what it received
 * and sent, its connections, and whether it may upload. Every note is the
 * engine's own words.
 */
export function TorrentCard({ torrent: t, colors = {}, expanded, onToggle, place }: CardProps) {
  const title = t.name ?? t.info_hash
  const moving = t.phase === 'downloading'
  const { action, sending, refused, press } = useAction(t.id, t.phase, actionFor(t.phase), controlTorrent)
  const [mine, setMine] = useState<boolean | null>(null)
  const open = expanded ?? mine ?? moving

  function toggle() {
    setMine(!open)
    onToggle?.(!open)
  }

  return (
    <section className={cardClass(t.phase)} aria-label={title}>
      <div className="tq-head">
        <div className="tq-ident">
          <div className="tq-tags">
            <span className="tq-badge">
              {moving && <span className="tq-dot" aria-hidden="true" />}
              <span data-testid="state">{t.phase}</span>
            </span>
            <span className="tq-id" data-testid="id">
              #{t.id}
            </span>
            <span className="tq-kind">torrent</span>
          </div>
          <h3 className={t.name === null ? 'tq-name hash' : 'tq-name'}>{title}</h3>
        </div>
        <div className="tq-actions">
          <TransferControls
            priority={t.priority}
            ended={t.phase === 'done' || t.phase === 'failed'}
            running={t.phase === 'finding' || t.phase === 'downloading'}
            onPriority={(priority) => setPriority('torrent', t.id, priority)}
            onRemove={() => controlTorrent(t.id, 'remove')}
          />
          {action !== null && (
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
          )}
        </div>
      </div>

      <div className="tq-progress">
        <PieceBar download={torrentBar(t)} colors={colors} />
        <span
          className={t.phase !== 'done' && t.length === null ? 'tq-percent unknown' : 'tq-percent'}
          data-testid="percent"
        >
          {t.phase === 'done' ? '100.0%' : percentText(t.done, t.length)}
        </span>
      </div>

      <Stats torrent={t} place={place} />

      {t.phase === 'finding' && (
        <p className="tq-words" data-testid="words">
          fetching its details from peers
        </p>
      )}
      {t.note !== null && (
        <p className={t.phase === 'failed' ? 'tq-note failure' : 'tq-note'} data-testid="note">
          {t.note}
        </p>
      )}
      {refused !== null && (
        <p className="tq-refused" role="alert">
          {refused}
        </p>
      )}

      <Distribution torrent={t} open={open} onToggle={toggle} colors={colors} />
    </section>
  )
}

/** The five figures under the bar. Speeds only while downloading, as the engine sends 0 otherwise. */
function Stats({ torrent: t, place }: { torrent: TorrentView; place?: number }) {
  const moving = t.phase === 'downloading'
  const speed = (bps: number) => {
    const [figure, unit] = mbps(bps).split(' ')
    return moving ? (
      <>
        {figure} <small>{unit}</small>
      </>
    ) : (
      '-'
    )
  }
  const whole = t.phase === 'done' ? (t.length ?? t.done) : t.length
  const got = t.phase === 'done' ? whole! : t.done

  return (
    <dl className="tq-stats">
      <Stat icon={<DriveIcon />} label="Fetched" id="progress">
        {whole === null && got === 0 ? (
          '-'
        ) : (
          <>
            <b>{formatBytes(got)}</b>
            <span className="tq-of"> / {whole === null ? '-' : formatBytes(whole)}</span>
          </>
        )}
      </Stat>
      <Stat icon={<GaugeIcon />} label="Down" id="down" tone={moving ? 'live' : undefined}>
        {speed(t.down_bps)}
      </Stat>
      <Stat icon={<UploadIcon />} label="Up" id="up">
        {speed(t.up_bps)}
      </Stat>
      <Stat icon={<ConnectionsIcon />} label="Peers" id="peers">
        {t.peers > 0 ? t.peers : '-'}
      </Stat>
      <Stat icon={<ClockIcon />} label="ETA" id="eta">
        {torrentEta(t, place)}
      </Stat>
    </dl>
  )
}

/** The card's foot, as the download card's: "Link distribution · 2 cables", a toggle for the list. */
function Distribution({
  torrent: t,
  open,
  onToggle,
  colors,
}: {
  torrent: TorrentView
  open: boolean
  onToggle: () => void
  colors: Record<string, string>
}) {
  const listId = useId()
  const rows = t.cables.filter((c) => c.received > 0 || c.sent > 0 || c.connections > 0)
  if (rows.length === 0) {
    return (
      <div className="tq-dist" data-testid="distribution">
        <span className="tq-dist-toggle static">Link distribution · no cables</span>
      </div>
    )
  }
  const received = rows.reduce((sum, c) => sum + c.received, 0)

  return (
    <div className={open ? 'tq-dist open' : 'tq-dist'} data-testid="distribution">
      <button type="button" className="tq-dist-toggle" aria-expanded={open} aria-controls={listId} onClick={onToggle}>
        {`Link distribution · ${rows.length} ${rows.length === 1 ? 'cable' : 'cables'}`}
        <ChevronIcon />
      </button>
      <ul id={listId} className="tq-shares" aria-label="Cables" hidden={!open}>
        {rows.map((c) => (
          <Share
            key={c.name}
            cable={c}
            color={colors[c.name] ?? 'var(--dim)'}
            received={received}
            moving={t.phase === 'downloading'}
          />
        ))}
      </ul>
    </div>
  )
}

/** One cable's part: its share of what came in, then what it sent, its connections, and if it may not upload. */
function Share({
  cable: c,
  color,
  received,
  moving,
}: {
  cable: TorrentCableView
  color: string
  received: number
  moving: boolean
}) {
  const share = received > 0 ? Math.min(100, (c.received * 100) / received) : 0
  return (
    <li className="tq-share" data-testid="cable" style={{ '--c': color } as CSSProperties}>
      <span className="tq-share-dot" aria-hidden="true" />
      <span className="tq-share-name" data-testid="cable-name">
        {c.name}
      </span>
      <span className="tq-share-bar" aria-hidden="true">
        <span data-testid="cable-share" style={{ width: `${share}%` }} />
      </span>
      <span className="tq-share-bytes" data-testid="cable-bytes">
        {formatBytes(c.received)}
      </span>
      <span className="tq-share-speed" data-testid="cable-speed">
        {moving ? mbps(c.bps) : ''}
      </span>
      <span className="tq-share-detail" data-testid="cable-detail">
        {`sent ${formatBytes(c.sent)} · ${c.connections} connection${c.connections === 1 ? '' : 's'}`}
        {!c.uploads && (
          <>
            {' · '}
            <b data-testid="upload-off">upload off: metered</b>
          </>
        )}
      </span>
    </li>
  )
}

/** The port, in words. */
function incoming({ state, cable }: TorrentsView['listening']): string {
  if (state === 'trying') return `trying on ${cable}`
  if (state === 'open') return `open on ${cable}`
  return state
}

/** The line above the torrents: where DHT and UDP run, and whether peers can call in. */
export function SwarmLine({ torrents }: { torrents: TorrentsView }) {
  const { udp_cable: cable, listening } = torrents
  return (
    <div className="tq-swarm" data-testid="swarm">
      <p className="tq-swarm-line">
        {cable === null ? 'DHT and UDP wait for a working cable' : `DHT and UDP on ${cable}`}
        {` · incoming: ${incoming(listening)}`}
      </p>
      {listening.why !== null && <p className="tq-swarm-why">{listening.why}</p>}
    </div>
  )
}
