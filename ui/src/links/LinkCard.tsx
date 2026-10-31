import { formatBytes } from '../downloads/format'
import { type Cable, joinNames, mbps, publicText, rtt } from './cables'

/**
 * One cable: its name and state, its speed, a beam on the scale every card
 * shares (the fastest cable fills its own), and every address on it with what
 * the internet sees it as.
 *
 * The state is a colour, a shape (both from the `cable <state>` classes) and
 * the engine's own word, always written out. A metered cable says so beside
 * it, in a word and a mark, never colour alone; once the cable has carried,
 * the data BARRAGE has used through it is beside its speed.
 */
export function LinkCard({ cable, fastestBps }: { cable: Cable; fastestBps: number | null }) {
  const share = cable.downBps !== null && fastestBps ? cable.downBps / fastestBps : 0
  const [figure, unit] = cable.downBps === null ? ['-', null] : mbps(cable.downBps).split(' ')
  const shared = cable.sharedWith.length > 0

  return (
    <section className={`panel cable ${cable.state}`} aria-label={cable.name}>
      <div className="top">
        <h2 className="name">{cable.name}</h2>
        <span className="state">
          <span className="mark" aria-hidden="true" />
          <span data-testid="state">
            {cable.rttMs === null ? cable.state : `${cable.state}, ${rtt(cable.rttMs)}`}
          </span>
          {cable.metered && (
            <span className="metered" data-testid="metered" title="Windows says this connection is metered">
              <svg viewBox="0 0 12 12" aria-hidden="true">
                <path d="M1.5 9a4.5 4.5 0 1 1 9 0" />
                <path d="M6 9 8.2 5.6" />
              </svg>
              metered
            </span>
          )}
        </span>
      </div>

      <div className="speedline">
        <div className={unit ? 'figure' : 'figure unknown'} data-testid="speed">
          {figure}
          {unit && (
            <>
              {' '}
              <small>{unit}</small>
            </>
          )}
        </div>
        {cable.used > 0 && (
          <div className="used" data-testid="used">
            used <b>{formatBytes(cable.used)}</b>
          </div>
        )}
      </div>

      <div className="beam" aria-hidden="true">
        <span
          data-testid="beam-fill"
          className={share === 0 ? 'empty' : undefined}
          style={{ width: `${share * 100}%` }}
        />
      </div>

      <ul className="addresses">
        {cable.links.map((link) => (
          <li key={link.id}>
            <span className="addr" data-testid="address">
              {link.address}
              {link.state !== cable.state && (
                <>
                  {' '}
                  <span className={`own-state ${link.state}`}>{link.state}</span>
                </>
              )}
            </span>
            <span className={shared ? 'public counted-once' : 'public'} data-testid="public">
              public {publicText(link)}
            </span>
          </li>
        ))}
      </ul>

      {shared && (
        <p className="shared-with" data-testid="shared">
          shared with {joinNames(cable.sharedWith)}
        </p>
      )}
    </section>
  )
}
