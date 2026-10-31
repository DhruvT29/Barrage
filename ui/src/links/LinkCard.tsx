import type { CSSProperties } from 'react'

import { formatBytes } from '../downloads/format'
import {
  addressSummary,
  type Cable,
  fractionText,
  interfaceKind,
  joinNames,
  liveShare,
  mbps,
  publicText,
  rtt,
  utilization,
} from './cables'
import { BoltMark, InterfaceIcon, MeteredMark } from './icons'
import { Sparkline } from './Sparkline'

/**
 * One cable in the download manager's left column: its icon, name and
 * addresses, its live downlink (or, while it carries nothing, its measured
 * speed) with a minute of it as a sparkline, how much of its measured speed
 * it is using, its share of what the downloads fetch, its ping, its state and
 * the data used through it, then every address with what the internet sees
 * it as.
 *
 * The state is a colour (the `ml-<state>` class, on the icon's dot and the
 * STATE figure) and always the engine's own word. Metered is a word and a
 * mark. Unknown is "-", never 0. The card's edge glows in its colour while it
 * carries a download.
 */
export function LinkCard({
  cable,
  color,
  liveBps,
  liveTotal,
  fastest,
  series,
}: {
  cable: Cable
  /** Its colour (palette.ts). */
  color: string
  /** What the downloads fetch through it now; null while it carries nothing. */
  liveBps: number | null
  /** What every download fetches now, all cables together; 0 while nothing downloads. */
  liveTotal: number
  /** Whether it has the highest measured speed of all the cables. */
  fastest: boolean
  /** The sparkline's samples: the live series while it carries, the measured one otherwise. */
  series: number[]
}) {
  const carrying = liveBps !== null
  const shown = carrying ? liveBps : cable.downBps
  const [figure, unit] = shown === null ? ['-', null] : mbps(shown).split(' ')
  const util = utilization(liveBps, cable.downBps)
  const shared = cable.sharedWith.length > 0
  const style = { '--c': color } as CSSProperties

  return (
    <section
      className={`ml-card ml-${cable.state}${carrying ? ' ml-carrying' : ''}`}
      aria-label={cable.name}
      style={style}
    >
      <div className="ml-head">
        <span className="ml-iconbox">
          <InterfaceIcon kind={interfaceKind(cable.name)} />
          <span className="ml-dot" aria-hidden="true" />
        </span>
        <div className="ml-id">
          <div className="ml-nameline">
            <h3 className="ml-name">{cable.name}</h3>
            {fastest && (
              <span className="ml-fastest" data-testid="fastest" title="The highest measured speed of all the cables">
                <BoltMark />
                Fastest
              </span>
            )}
          </div>
          <p className="ml-summary" data-testid="summary">
            {addressSummary(cable.links)}
          </p>
        </div>
        {cable.metered && (
          <span className="ml-metered" data-testid="metered" title="Windows says this connection is metered">
            <MeteredMark />
            metered
          </span>
        )}
      </div>

      <div className="ml-speed">
        <div>
          <p className="ml-label" data-testid="figure-label">
            {carrying ? 'Live downlink' : 'Measured'}
          </p>
          <p className={unit ? 'ml-figure' : 'ml-figure ml-unknown'} data-testid="speed">
            {figure}
            {unit && (
              <>
                {' '}
                <small>{unit}</small>
              </>
            )}
          </p>
        </div>
        <Sparkline series={series} color={color} />
      </div>

      <div className="ml-util">
        <div className="ml-utilhead">
          <span className="ml-label">Link utilization</span>
          <span className="ml-utiltext" data-testid="utilization">
            {carrying ? `${fractionText(util)} / ${cable.downBps === null ? '-' : mbps(cable.downBps)}` : 'not carrying'}
          </span>
        </div>
        <div className="ml-bar" aria-hidden="true">
          <span data-testid="util-fill" style={{ width: `${Math.min(util ?? 0, 1) * 100}%` }} />
        </div>
      </div>

      <dl className="ml-stats">
        <div data-testid="stat-share">
          <dt>Share</dt>
          <dd>{fractionText(liveShare(liveBps, liveTotal))}</dd>
        </div>
        <div data-testid="stat-ping">
          <dt>Ping</dt>
          <dd>{rtt(cable.rttMs)}</dd>
        </div>
        <div data-testid="stat-state">
          <dt>State</dt>
          <dd className="ml-state" data-testid="state">
            {cable.state}
          </dd>
        </div>
        <div data-testid="stat-used">
          <dt>Used</dt>
          <dd>{cable.used > 0 ? formatBytes(cable.used) : '-'}</dd>
        </div>
      </dl>

      <ul className="ml-addresses">
        {cable.links.map((link) => (
          <li key={link.id}>
            <span data-testid="address">
              {link.address}
              {link.state !== cable.state && (
                <>
                  {' '}
                  <span className={`ml-own ml-${link.state}`}>{link.state}</span>
                </>
              )}
            </span>
            <span className="ml-sep" aria-hidden="true">
              ·
            </span>
            <span data-testid="public">public {publicText(link)}</span>
          </li>
        ))}
      </ul>

      {shared && (
        <p className="ml-shared" data-testid="shared">
          shared with {joinNames(cable.sharedWith)}
        </p>
      )}
    </section>
  )
}
