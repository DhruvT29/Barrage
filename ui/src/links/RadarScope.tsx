import { type KeyboardEvent, useEffect, useRef, useState } from 'react'

import type { DownloadsView } from '../api/types'
import { type Cable, liveBps, mbps, rtt } from './cables'
import { count } from './messages'
import {
  bearing,
  labelPlace,
  nodeLabel,
  nodeName,
  nodeRadius,
  polar,
  RANGE_MS,
  reach,
  RINGS_MS,
  TONE_COLOR,
  tone,
} from './radarModel'

/** The drawing's size, in its own units; it scales with the panel. */
const SIZE = 640
const C = SIZE / 2
/** The edge: a round trip of RANGE_MS. */
const R = 250
/** A round trip of 0: clear of the hub and its label. */
const INNER = 58
/** One turn of the sweep, in milliseconds; the CSS animation uses the same. */
export const SWEEP_MS = 4000

/** Whether the viewer asked for less motion; false where it cannot be asked (tests). */
function reducedMotion(): boolean {
  return typeof window.matchMedia === 'function' && window.matchMedia('(prefers-reduced-motion: reduce)').matches
}

/**
 * The sweep's azimuth, written straight into its readout ten times a second
 * (no re-render). Purely cosmetic: it follows the sweep, which is drawn by a
 * CSS animation started in phase with the same clock.
 */
function useAzimuth() {
  const readout = useRef<HTMLSpanElement>(null)
  const [delay] = useState(() => `-${Math.round(performance.now() % SWEEP_MS)}ms`)
  useEffect(() => {
    if (reducedMotion()) return
    const timer = setInterval(() => {
      const deg = Math.floor(((performance.now() % SWEEP_MS) / SWEEP_MS) * 360)
      if (readout.current) readout.current.textContent = `${String(deg).padStart(3, '0')}°`
    }, 100)
    return () => clearInterval(timer)
  }, [])
  return { readout, delay }
}

/** The bearing marks round the edge: a short one every 5 degrees, a long one with its figure every 30. */
function Bearings() {
  const marks = []
  for (let deg = 0; deg < 360; deg += 5) {
    const major = deg % 30 === 0
    const a = polar(C, C, R + (major ? 6 : 9), deg)
    const b = polar(C, C, R + (major ? 18 : 14), deg)
    marks.push(<line key={deg} className={major ? 'rd-tick rd-major' : 'rd-tick'} x1={a.x} y1={a.y} x2={b.x} y2={b.y} />)
    if (major) {
      const t = polar(C, C, R + 34, deg)
      marks.push(
        <text key={`t${deg}`} className="rd-deg" x={t.x} y={t.y} textAnchor="middle" dominantBaseline="central">
          {String(deg).padStart(3, '0')}
        </text>,
      )
    }
  }
  return <g aria-hidden="true">{marks}</g>
}

/** The corner brackets round the selected node, as the design locks a target. */
function Brackets({ x, y }: { x: number; y: number }) {
  const d = 17
  const l = 6
  const corners = [
    [-1, -1],
    [1, -1],
    [1, 1],
    [-1, 1],
  ]
  return (
    <g className="rd-brackets" aria-hidden="true">
      {corners.map(([sx, sy]) => (
        <path
          key={`${sx}${sy}`}
          d={`M${x + sx * d} ${y + sy * (d - l)} V${y + sy * d} H${x + sx * (d - l)}`}
        />
      ))}
    </g>
  )
}

/**
 * The radar: this PC at the centre, one node per cable spread evenly round
 * it in table order, each as far out as its round trip (the rings mark 25,
 * 50, 75 and 100 ms; beyond, or before the first probe, on the edge, hollow
 * when there is no round trip yet). A node is coloured by its state and named
 * with it in words; a cable carrying a download has a dashed line to the hub
 * and a pulse. A click, Enter or Space selects a node.
 */
export function RadarScope({
  list,
  downloads,
  selected,
  onSelect,
  empty,
  known,
}: {
  /** The cables to draw, in table order. */
  list: Cable[]
  /** Whether the engine has sent a table: before it, how many are in range is unknown. */
  known: boolean
  downloads: DownloadsView | null
  selected: string | null
  onSelect: (name: string) => void
  /** What to say in place of the nodes when there are none. */
  empty: string | null
}) {
  const { readout, delay } = useAzimuth()

  const key = (name: string) => (e: KeyboardEvent) => {
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault()
      onSelect(name)
    }
  }

  const nodes = list.map((cable, i) => {
    const deg = bearing(i, list.length)
    const r = nodeRadius(cable.rttMs, R, INNER)
    const p = polar(C, C, r, deg)
    const carrying = liveBps(downloads, cable.name) !== null
    const place = labelPlace(deg, reach(cable.rttMs))
    return { cable, deg, p, place, carrying, color: TONE_COLOR[tone(cable.state)] }
  })
  const chosen = nodes.find((n) => n.cable.name === selected)

  return (
    <div className="rd-scope">
      <div className="rd-readout rd-azimuth" aria-hidden="true">
        <span className="rd-eyebrow">Azimuth</span>
        <span className="rd-readfig" ref={readout}>
          000°
        </span>
      </div>
      <div className="rd-readout rd-range">
        <span className="rd-eyebrow">In range</span>
        <span className="rd-readfig" data-testid="in-range">
          {count(known ? list.length : null)} <small>{list.length === 1 ? 'node' : 'nodes'}</small>
        </span>
      </div>

      <div className="rd-sweep" style={{ animationDelay: delay }} aria-hidden="true" />

      <svg className="rd-svg" viewBox={`0 0 ${SIZE} ${SIZE}`} role="group" aria-label="Radar">
        <defs>
          <radialGradient id="rd-floor">
            <stop offset="0%" className="rd-floor-in" />
            <stop offset="100%" className="rd-floor-out" />
          </radialGradient>
        </defs>
        <circle cx={C} cy={C} r={R} fill="url(#rd-floor)" className="rd-edge" />
        <g aria-hidden="true">
          <line className="rd-cross" x1={C - R} y1={C} x2={C + R} y2={C} />
          <line className="rd-cross" x1={C} y1={C - R} x2={C} y2={C + R} />
          {RINGS_MS.map((ms) => {
            const r = nodeRadius(ms, R, INNER)
            return (
              <g key={ms}>
                {ms < RANGE_MS && <circle className="rd-ring" cx={C} cy={C} r={r} />}
                <text className="rd-ringlabel" x={C + 6} y={C - r + 13}>
                  {ms} ms
                </text>
              </g>
            )
          })}
        </g>
        <Bearings />

        {nodes
          .filter((n) => n.carrying)
          .map((n) => (
            <line
              key={`link-${n.cable.name}`}
              className="rd-beam"
              style={{ stroke: n.color }}
              x1={C}
              y1={C}
              x2={n.p.x}
              y2={n.p.y}
              aria-hidden="true"
            />
          ))}

        <g className="rd-hub" aria-hidden="true">
          <circle cx={C} cy={C} r={22} className="rd-hubring" />
          <circle cx={C} cy={C} r={8} className="rd-hubcore" />
          <text x={C} y={C + 38} textAnchor="middle" className="rd-hublabel">
            THIS PC
          </text>
        </g>

        {nodes.map(({ cable, p, place, carrying, color }) => {
          const isSelected = cable.name === selected
          const hollow = cable.rttMs === null
          return (
            <g
              key={cable.name}
              className={`rd-node${isSelected ? ' rd-selected' : ''}`}
              style={{ color }}
              role="button"
              tabIndex={0}
              aria-label={nodeLabel(cable, carrying)}
              aria-pressed={isSelected}
              onClick={() => onSelect(cable.name)}
              onKeyDown={key(cable.name)}
            >
              <circle className="rd-hit" cx={p.x} cy={p.y} r={22} />
              <circle className="rd-focusring" cx={p.x} cy={p.y} r={24} />
              {carrying && <circle className="rd-pulse" cx={p.x} cy={p.y} r={8} />}
              <circle className={hollow ? 'rd-dot rd-hollow' : 'rd-dot'} cx={p.x} cy={p.y} r={7} />
              <title>{cable.name}</title>
              <text className="rd-nname" x={p.x + place.dx} y={p.y + place.nameDy} textAnchor={place.anchor}>
                {nodeName(cable.name)}
              </text>
              <text className="rd-nfig" x={p.x + place.dx} y={p.y + place.figDy} textAnchor={place.anchor}>
                {rtt(cable.rttMs)} · {cable.downBps === null ? '-' : mbps(cable.downBps)}
              </text>
            </g>
          )
        })}

        {chosen && <Brackets x={chosen.p.x} y={chosen.p.y} />}
      </svg>

      {empty && <p className="rd-scopeempty">{empty}</p>}
    </div>
  )
}
