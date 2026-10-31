import { useState, type KeyboardEvent, type PointerEvent } from 'react'

import { HISTORY_LEN } from '../api/history'
import { mbps } from '../links/cables'

/** One cable's series, oldest first, in bits per second, and its colour. */
export type ChartSeries = { name: string; color: string; values: number[] }

const W = 600
const H = 150
const PAD_TOP = 10

type Point = [number, number]

/**
 * A smooth line through `points` that never overshoots them (a monotone
 * cubic, Fritsch and Carlson), as path commands after the first point. A
 * speed drawn above or below what was sampled would be a made-up figure.
 */
function curve(points: Point[]): string {
  const n = points.length
  if (n < 2) return ''
  const dx: number[] = []
  const slope: number[] = []
  for (let i = 0; i < n - 1; i++) {
    dx.push(points[i + 1][0] - points[i][0])
    slope.push((points[i + 1][1] - points[i][1]) / (dx[i] || 1))
  }
  const tangent: number[] = [slope[0]]
  for (let i = 1; i < n - 1; i++) {
    tangent.push(slope[i - 1] * slope[i] <= 0 ? 0 : (slope[i - 1] + slope[i]) / 2)
  }
  tangent.push(slope[n - 2])
  for (let i = 0; i < n - 1; i++) {
    if (slope[i] === 0) {
      tangent[i] = 0
      tangent[i + 1] = 0
      continue
    }
    const a = tangent[i] / slope[i]
    const b = tangent[i + 1] / slope[i]
    const h = a * a + b * b
    if (h > 9) {
      const t = 3 / Math.sqrt(h)
      tangent[i] = t * a * slope[i]
      tangent[i + 1] = t * b * slope[i]
    }
  }
  let d = ''
  for (let i = 0; i < n - 1; i++) {
    const [x0, y0] = points[i]
    const [x1, y1] = points[i + 1]
    const third = dx[i] / 3
    d += ` C${x0 + third},${y0 + tangent[i] * third} ${x1 - third},${y1 - tangent[i + 1] * third} ${x1},${y1}`
  }
  return d
}

/** A speed for the tooltip, or "-" for a 0 that means not measured. */
function figure(bps: number, zeroUnknown: boolean): string {
  return bps === 0 && zeroUnknown ? '-' : mbps(bps)
}

/** "now", "1 s ago", "12 s ago": samples are one second apart. */
function ago(stepsBack: number): string {
  return stepsBack === 0 ? 'now' : `${stepsBack} s ago`
}

/**
 * The last minute as stacked bands, one per cable in its colour, the newest
 * second at the right edge: a minute of history is always a minute wide, so
 * the first seconds after the page opens fill in from the right. Pointing
 * at it, or focusing it and using the arrow keys, shows each cable's figure
 * at that second.
 */
export function SpeedChart({
  series,
  samples,
  label,
  quiet,
  zeroUnknown,
}: {
  series: ChartSeries[]
  /** Seconds sampled so far, at most a minute's worth. */
  samples: number
  label: string
  /** What to say when nothing moved in the minute. */
  quiet: string
  /**
   * Whether a 0 means "not measured" rather than a real 0: the measured
   * series hold 0 until a cable is measured, so the tooltip shows "-".
   */
  zeroUnknown: boolean
}) {
  const [index, setIndex] = useState<number | null>(null)
  const n = Math.max(samples, ...series.map((s) => s.values.length))
  const step = W / (HISTORY_LEN - 1)
  const x = (i: number) => W - (n - 1 - i) * step

  // Stacked from the first cable up: each band sits on the ones before it.
  const tops: number[][] = []
  let below = new Array<number>(n).fill(0)
  for (const s of series) {
    const top = below.map((b, i) => b + (s.values[i] ?? 0))
    tops.push(top)
    below = top
  }
  const totals = below
  const peak = Math.max(0, ...totals)
  const scale = peak > 0 ? peak * 1.12 : 1
  const y = (v: number) => H - (v / scale) * (H - PAD_TOP)

  const bands = series.map((s, k) => {
    const upper: Point[] = tops[k].map((v, i) => [x(i), y(v)])
    const lower: Point[] = (k === 0 ? new Array<number>(n).fill(0) : tops[k - 1]).map((v, i) => [x(i), y(v)])
    const reversed = [...lower].reverse()
    const line = n > 0 ? `M${upper[0][0]},${upper[0][1]}${curve(upper)}` : ''
    const area =
      n > 0 ? `${line} L${reversed[0][0]},${reversed[0][1]}${curve(reversed)} Z` : ''
    return { ...s, line, area }
  })

  const shown = index === null || n === 0 || series.length === 0 ? null : Math.min(Math.max(index, 0), n - 1)

  function pointAt(event: PointerEvent<HTMLDivElement>) {
    if (n === 0) return
    const box = event.currentTarget.getBoundingClientRect()
    if (box.width === 0) return
    const atX = ((event.clientX - box.left) / box.width) * W
    setIndex(Math.round((atX - x(0)) / step))
  }

  function onKey(event: KeyboardEvent<HTMLDivElement>) {
    if (n === 0) return
    const from = shown ?? n - 1
    const to =
      event.key === 'ArrowLeft' ? from - 1
      : event.key === 'ArrowRight' ? from + 1
      : event.key === 'Home' ? 0
      : event.key === 'End' ? n - 1
      : null
    if (to === null) return
    event.preventDefault()
    setIndex(Math.min(Math.max(to, 0), n - 1))
  }

  const leftPct = shown === null ? 0 : (x(shown) / W) * 100
  const flip = leftPct > 58

  return (
    <div
      className="hero-chart"
      role="group"
      aria-label={`${label}, last minute, by cable. Arrow keys read each second.`}
      tabIndex={0}
      onPointerMove={pointAt}
      onPointerLeave={() => setIndex(null)}
      onFocus={() => setIndex((i) => i ?? n - 1)}
      onBlur={() => setIndex(null)}
      onKeyDown={onKey}
    >
      <svg viewBox={`0 0 ${W} ${H}`} preserveAspectRatio="none" aria-hidden="true" focusable="false">
        {[0.25, 0.5, 0.75].map((f) => (
          <line key={f} className="hero-chart-grid" x1={0} x2={W} y1={H * f} y2={H * f} />
        ))}
        {bands.map((b) => (
          <g key={b.name} data-testid="band" style={{ color: b.color }}>
            <path className="hero-band" d={b.area} />
            <path className="hero-band-line" d={b.line} />
          </g>
        ))}
        <line className="hero-chart-base" x1={0} x2={W} y1={H - 0.5} y2={H - 0.5} />
      </svg>

      {peak === 0 && (
        <p className="hero-chart-empty">{n < 2 ? 'Gathering the first seconds' : quiet}</p>
      )}

      {shown !== null && (
        <>
          <span className="hero-cursor" style={{ left: `${leftPct}%` }} aria-hidden="true">
            {series.map((s, k) => (
              <i key={s.name} style={{ top: `${(y(tops[k][shown]) / H) * 100}%`, color: s.color }} />
            ))}
          </span>
          <div
            className={flip ? 'hero-tip flip' : 'hero-tip'}
            style={{ left: `${leftPct}%` }}
            data-testid="chart-tip"
          >
            <div className="hero-tip-head">
              <span>{label}</span>
              <span>{ago(n - 1 - shown)}</span>
            </div>
            <ul>
              {[...series].reverse().map((s) => (
                <li key={s.name} style={{ color: s.color }}>
                  <span>{s.name}</span>
                  <b>{figure(s.values[shown] ?? 0, zeroUnknown)}</b>
                </li>
              ))}
            </ul>
            <div className="hero-tip-total">
              <span>Total</span>
              <b>{figure(totals[shown] ?? 0, zeroUnknown)}</b>
            </div>
          </div>
        </>
      )}
    </div>
  )
}
