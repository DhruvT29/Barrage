import type { CableShare, DownloadState, DownloadView } from '../api/types'
import { mbps } from '../links/cables'
import { formatBytes, percent } from './format'

/** What a card says in place of a speed, while it has none to show. */
const WAITING_WORDS: Partial<Record<DownloadState, string>> = {
  starting: 'finding the file',
  waiting: 'no cable to fetch it through yet',
}

/**
 * One download: its name (its address until the file is found), its state,
 * its speed while it downloads, how far it has got, and a row per cable with
 * what that cable carried. Every note is the engine's own words.
 *
 * The state is a colour, a shape (both from the `dl <state>` classes) and
 * the engine's word, always written out.
 */
export function DownloadCard({ download: d }: { download: DownloadView }) {
  const title = d.name ?? d.url
  const moving = d.state === 'downloading'
  const words = WAITING_WORDS[d.state]
  const progress = progressOf(d)
  // A cable's note that repeats the download's own says nothing new, and a
  // cable that carried nothing, holds nothing open and has nothing new to
  // say is left out.
  const rows = d.cables
    .map((c) => ({ ...c, notes: c.notes.filter((n) => n !== d.note) }))
    .filter((c) => c.bytes > 0 || c.connections > 0 || c.notes.length > 0)

  return (
    <section className={`panel dl ${d.state}`} aria-label={title}>
      <div className="top">
        <h2 className="name">{title}</h2>
        <span className="state">
          <span className="mark" aria-hidden="true" />
          <span data-testid="state">{d.state}</span>
        </span>
      </div>

      {(moving || words || progress) && (
        <div className="figures">
          {moving && <Speed bps={d.bps} />}
          {words && (
            <span className="words" data-testid="words">
              {words}
            </span>
          )}
          {progress && (
            <span className="progress">
              <span data-testid="progress">{progress.text}</span>
              {progress.percent !== null && <b data-testid="percent">{progress.percent}%</b>}
            </span>
          )}
        </div>
      )}
      {progress && progress.fill !== null && (
        <div className="beam" aria-hidden="true">
          <span data-testid="progress-fill" style={{ width: `${progress.fill}%` }} />
        </div>
      )}

      {d.note !== null && (
        <p className="note" data-testid="note">
          {d.note}
        </p>
      )}

      {rows.length > 0 && (
        <ul className="shares" aria-label="Cables">
          {rows.map((c) => (
            <Share key={c.name} cable={c} done={d.done} moving={moving} />
          ))}
        </ul>
      )}

      <div className="foot" data-testid="foot">
        {d.name !== null && <span className="url">{d.url}</span>}
        <span className="id">download {d.id}</span>
      </div>
    </section>
  )
}

type Progress = {
  text: string
  /** Whole percent done; null without a length, or once done. */
  percent: number | null
  /** How far along the beam, 0 to 100; null without a length to measure against. */
  fill: number | null
}

/** How far a download has got; null while there is nothing to say. */
function progressOf(d: DownloadView): Progress | null {
  if (d.state === 'done') return { text: formatBytes(d.length ?? d.done), percent: null, fill: 100 }
  if (d.length !== null) {
    return {
      text: `${formatBytes(d.done)} of ${formatBytes(d.length)}`,
      percent: percent(d.done, d.length),
      fill: d.length === 0 ? 100 : Math.min(100, (d.done * 100) / d.length),
    }
  }
  if (d.done > 0) return { text: formatBytes(d.done), percent: null, fill: null }
  return null
}

function Speed({ bps }: { bps: number }) {
  const [figure, unit] = mbps(bps).split(' ')
  return (
    <span className="figure" data-testid="speed">
      {figure} <small>{unit}</small>
    </span>
  )
}

/** One cable's part: its share of the bytes so far, and while it downloads its speed and connections. */
function Share({ cable: c, done, moving }: { cable: CableShare; done: number; moving: boolean }) {
  const share = done > 0 ? Math.min(100, (c.bytes * 100) / done) : 0
  const out = c.bytes === 0 && c.connections === 0
  // A cable with nothing open and nothing measured has no speed to give.
  const speed = !moving ? '' : c.connections > 0 || c.bps > 0 ? mbps(c.bps) : '-'

  return (
    <li className={out ? 'out' : undefined} data-testid="cable">
      <span className="share-name" data-testid="cable-name">
        {c.name}
      </span>
      <span className="share-bar" aria-hidden="true">
        <span data-testid="cable-share" style={{ width: `${share}%` }} />
      </span>
      <span className="share-bytes" data-testid="cable-bytes">
        {formatBytes(c.bytes)}
      </span>
      <span className="share-speed" data-testid="cable-speed">
        {speed}
      </span>
      <span className="share-conns" data-testid="cable-connections">
        {moving && c.connections > 0 ? `${c.connections} connection${c.connections === 1 ? '' : 's'}` : ''}
      </span>
      {c.notes.map((n) => (
        <span className="share-note" data-testid="cable-note" key={n}>
          {n}
        </span>
      ))}
    </li>
  )
}
