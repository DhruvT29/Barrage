import { type Cable, joinNames, mbps, publicText, rtt } from './cables'

/**
 * One cable: its name and state, its speed, a beam on the scale every card
 * shares (the fastest cable fills its own), and every address on it with what
 * the internet sees it as.
 *
 * The state is a colour, a shape (both from the `cable <state>` classes) and
 * the engine's own word, always written out.
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
        </span>
      </div>

      <div className={unit ? 'figure' : 'figure unknown'} data-testid="speed">
        {figure}
        {unit && (
          <>
            {' '}
            <small>{unit}</small>
          </>
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
