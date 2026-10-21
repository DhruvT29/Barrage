import type { CombinedView, DownloadsView, LiveSpeed } from '../api/types'
import { joinNames, mbps } from './cables'

/**
 * The page's main figure, across the top of both tabs. While anything
 * downloads it is the live download speed, a segment per cable carrying
 * data; otherwise the combined speed available, a segment per independent
 * connection. The engine works out both (`api/view.rs`); this shows them,
 * with a note saying what was counted.
 */
export function Combined({ combined, downloads }: { combined: CombinedView; downloads: DownloadsView | null }) {
  // Running is a download's state, not its speed: one that stalls for a
  // moment keeps the hero on it rather than flicking back and forth.
  const running = downloads?.list.filter((d) => d.state === 'downloading').length ?? 0
  if (downloads && running > 0) return <Downloading live={downloads.live} running={running} />
  return <Available combined={combined} />
}

function Downloading({ live, running }: { live: LiveSpeed; running: number }) {
  const through = live.parts.length === 0 ? 'nothing arriving right now' : `through ${count(live.parts.length, 'cable')}`
  return (
    <Hero
      label="Downloading at"
      bps={live.bps}
      from={`${count(running, 'download')}, ${through}`}
      parts={live.parts}
      note="What every download is fetching right now, added up by cable. With nothing downloading, this shows the combined speed available."
    />
  )
}

function Available({ combined }: { combined: CombinedView }) {
  const { bps, parts, not_counted } = combined
  const notes: string[] = ["The sum of each working connection's measured speed."]
  for (const part of parts) {
    const n = part.shared_with.length
    if (n > 0) {
      notes.push(
        `${joinNames(part.shared_with)} ${n === 1 ? 'shares' : 'share'} ${part.name}'s connection, ` +
          `so ${n === 1 ? 'it is' : 'they are'} counted once with it.`,
      )
    }
  }
  if (not_counted.length > 0) {
    notes.push(`Not counted: ${not_counted.map((c) => `${c.name} (${c.why})`).join(', ')}.`)
  }

  return (
    <Hero
      label="Combined speed available"
      bps={parts.length > 0 ? bps : null}
      from={
        parts.length === 0
          ? 'nothing measured yet'
          : parts.length === 1
            ? 'from 1 connection'
            : `from ${parts.length} independent connections`
      }
      parts={parts}
      note={notes.join(' ')}
    />
  )
}

/** "1 cable", "2 cables". */
function count(n: number, thing: string): string {
  return `${n} ${thing}${n === 1 ? '' : 's'}`
}

/**
 * A total in Mbps, a segment per part on one beam, and a note. With `bps`
 * null nothing is known yet: the total is "-", never 0.
 */
function Hero({
  label,
  bps,
  from,
  parts,
  note,
}: {
  label: string
  bps: number | null
  from: string
  parts: { name: string; bps: number }[]
  note: string
}) {
  const [figure, unit] = bps === null ? ['-', null] : mbps(bps).split(' ')

  return (
    <section className="panel hero" aria-label={label}>
      <div>
        <div className="label">{label}</div>
        <div className={unit ? 'total' : 'total unknown'} data-testid="total">
          {figure}
          {unit && (
            <>
              {' '}
              <small>{unit}</small>
            </>
          )}
        </div>
        <div className="from">{from}</div>
      </div>

      {bps !== null && bps > 0 && parts.length > 0 && (
        <div className="split">
          <div className="stack" aria-hidden="true">
            {parts.map((p) => (
              <span key={p.name} data-testid="segment" style={{ width: `${(p.bps / bps) * 100}%` }} />
            ))}
          </div>
          <div className="parts">
            {parts.map((p) => (
              <span key={p.name} data-testid="part" style={{ width: `${(p.bps / bps) * 100}%` }}>
                <b>{p.name}</b> {mbps(p.bps).split(' ')[0]}
              </span>
            ))}
          </div>
        </div>
      )}

      <p className="note" data-testid="note">
        {note}
      </p>
    </section>
  )
}
