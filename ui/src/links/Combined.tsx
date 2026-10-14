import type { CombinedView } from '../api/types'
import { joinNames, mbps } from './cables'

/**
 * The combined speed available: the page's main figure. The engine works it
 * out (`api/view.rs`); this shows it, with a segment per independent
 * connection and a note saying what was counted. In phase 3 the same place
 * shows the live download speed.
 */
export function Combined({ combined }: { combined: CombinedView }) {
  const { bps, parts, not_counted } = combined
  const measured = parts.length > 0
  const [figure, unit] = measured ? mbps(bps).split(' ') : ['-', null]

  const notes: string[] = []
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
    <section className="panel hero" aria-label="Combined speed available">
      <div>
        <div className="label">Combined speed available</div>
        <div className={unit ? 'total' : 'total unknown'} data-testid="total">
          {figure}
          {unit && (
            <>
              {' '}
              <small>{unit}</small>
            </>
          )}
        </div>
        <div className="from">
          {!measured
            ? 'nothing measured yet'
            : parts.length === 1
              ? 'from 1 connection'
              : `from ${parts.length} independent connections`}
        </div>
      </div>

      {measured && (
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
        The sum of each working connection&apos;s measured speed.
        {notes.map((n) => ` ${n}`).join('')}
      </p>
    </section>
  )
}
