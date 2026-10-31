import type { CSSProperties, ReactNode } from 'react'

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
import { InterfaceIcon } from './icons'
import { RANGE_MS, reach, stateWords, TONE_COLOR, tone } from './radarModel'

/** A small progress ring with its label, figure and a faint line under it. */
function RingTile({
  id,
  label,
  value,
  sub,
  fraction,
  color,
}: {
  id: string
  label: string
  value: string
  sub: string
  /** 0 to 1; null draws an empty ring. */
  fraction: number | null
  color: string
}) {
  const r = 15
  const length = 2 * Math.PI * r
  const shown = Math.min(Math.max(fraction ?? 0, 0), 1)
  return (
    <div className="rd-tile" data-testid={`ring-${id}`}>
      <svg viewBox="0 0 40 40" width="40" height="40" aria-hidden="true" className="rd-ringsvg">
        <circle cx="20" cy="20" r={r} className="rd-ringtrack" />
        {shown > 0 && (
          <circle
            cx="20"
            cy="20"
            r={r}
            className="rd-ringfill"
            style={{ stroke: color, strokeDasharray: `${shown * length} ${length}` }}
          />
        )}
      </svg>
      <div>
        <p className="rd-eyebrow">{label}</p>
        <p className="rd-tilefig">{value}</p>
        <p className="rd-tilesub">{sub}</p>
      </div>
    </div>
  )
}

/**
 * The selected cable, inspected: its state in words, its live throughput
 * against its measured capacity, its share of what the downloads fetch, its
 * latency, the data used through it and whether Windows calls it metered,
 * then every fact the engine has about it. The panel's edge glows in its
 * state's colour. No action buttons: the engine has none for a cable.
 */
export function RadarDetail({
  cable,
  color,
  liveBps,
  liveTotal,
  usedTotal,
  downloadsKnown,
  empty,
}: {
  cable: Cable | null
  /** Its colour (palette.ts). */
  color: string
  /** What the downloads fetch through it now; null while it carries nothing. */
  liveBps: number | null
  /** What every download fetches now; 0 while nothing downloads. */
  liveTotal: number
  /** The data used through every cable together. */
  usedTotal: number
  /** Whether the engine has said what downloads; before it has, the live speed is unknown. */
  downloadsKnown: boolean
  /** What to say when there is no cable to inspect. */
  empty: string | null
}) {
  if (cable === null) {
    return (
      <section className="rd-detail rd-panel" aria-label="Selected cable">
        <p className="rd-eyebrow">Inspector</p>
        {/* A cable is always selected while any is shown, so no cable means none to show. */}
        <p className="rd-quiet">{empty ? 'Nothing to inspect.' : "Waiting for the engine's first table."}</p>
      </section>
    )
  }

  const carrying = liveBps !== null
  const t = tone(cable.state)
  const actual = carrying ? mbps(liveBps) : downloadsKnown ? mbps(0) : null
  const util = utilization(liveBps, cable.downBps)
  const style = { '--c': color, '--tone': TONE_COLOR[t] } as CSSProperties

  const facts: [string, ReactNode][] = [['Interface', cable.name]]
  cable.links.forEach((link, i) => {
    const n = cable.links.length > 1 ? ` ${i + 1}` : ''
    facts.push([
      `Address${n}`,
      <>
        {link.address}
        {link.state !== cable.state && <span className={`rd-own rd-tone-${tone(link.state)}`}> {link.state}</span>}
      </>,
    ])
    facts.push([`Public${n}`, publicText(link)])
  })
  facts.push(['Family', addressSummary(cable.links).split(' · ')[1] ?? '-'])
  facts.push(['State', <span className={`rd-tone-${t}`}>{cable.state}</span>])
  facts.push(['Shared with', cable.sharedWith.length ? joinNames(cable.sharedWith) : 'none'])

  return (
    <section className={`rd-detail rd-panel rd-tone-edge-${t}`} aria-label="Selected cable" style={style}>
      <div className="rd-dhead">
        <span className="rd-iconbox">
          <InterfaceIcon kind={interfaceKind(cable.name)} size={22} />
        </span>
        <div>
          <span className="rd-badge" data-testid="badge">
            {stateWords(cable.state, carrying)}
          </span>
          <h2 className="rd-dname">{cable.name}</h2>
          <p className="rd-dsub" data-testid="summary">
            {addressSummary(cable.links)}
          </p>
        </div>
      </div>

      <div className="rd-throughput">
        <div className="rd-tprow">
          <div>
            <p className="rd-eyebrow">Actual throughput</p>
            <p className="rd-tpfig" data-testid="actual">
              {actual === null ? '-' : (
                <>
                  {actual.split(' ')[0]} <small>Mbps</small>
                </>
              )}
            </p>
          </div>
          <div className="rd-cap">
            <p className="rd-eyebrow">Capacity</p>
            <p className="rd-capfig" data-testid="capacity">
              {cable.downBps === null ? '-' : mbps(cable.downBps)}
            </p>
          </div>
        </div>
        <div className="rd-tpbar" aria-hidden="true">
          <span style={{ width: `${Math.min(util ?? 0, 1) * 100}%` }} />
        </div>
        {!carrying && <p className="rd-idle">Idle: not carrying a download right now.</p>}
      </div>

      <div className="rd-tiles">
        <RingTile
          id="share"
          label="Share"
          value={fractionText(liveShare(liveBps, liveTotal))}
          sub="of the live total"
          fraction={liveShare(liveBps, liveTotal)}
          color={color}
        />
        <RingTile
          id="latency"
          label="Latency"
          value={rtt(cable.rttMs)}
          sub={`range ${RANGE_MS} ms`}
          fraction={cable.rttMs === null ? null : reach(cable.rttMs)}
          color={TONE_COLOR[t]}
        />
        <RingTile
          id="used"
          label="Data used"
          value={cable.used > 0 ? formatBytes(cable.used) : '-'}
          sub="since start"
          fraction={usedTotal > 0 ? cable.used / usedTotal : null}
          color={color}
        />
        <RingTile
          id="metered"
          label="Metered"
          value={cable.metered ? 'yes' : 'no'}
          sub="as Windows says"
          fraction={cable.metered ? 1 : null}
          color="var(--amber)"
        />
      </div>

      <dl className="rd-facts" data-testid="facts">
        {facts.map(([label, value]) => (
          <div key={label}>
            <dt>{label}</dt>
            <dd>{value}</dd>
          </div>
        ))}
      </dl>
    </section>
  )
}
