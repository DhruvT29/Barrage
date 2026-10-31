import { useId } from 'react'

/**
 * A minute of one speed as a line with a soft fill under it, scaled to its
 * own highest sample: the shape of the speed, not a scale to read off. A
 * series that is all zeros (nothing measured, nothing carried) draws only a
 * faint baseline, never a line at 0 that would read as a figure.
 */
export function Sparkline({
  series,
  color,
  width = 112,
  height = 40,
}: {
  series: number[]
  color: string
  width?: number
  height?: number
}) {
  // Safe in url(#...) whatever characters React's ids use.
  const fill = `ml-spark${useId().replace(/[^a-zA-Z0-9_-]/g, '')}`
  const max = Math.max(0, ...series)
  const pad = 3
  const base = height - 1

  let line = ''
  let area = ''
  if (max > 0 && series.length > 1) {
    const step = width / (series.length - 1)
    const points = series.map((v, i) => [i * step, base - (v / max) * (height - pad * 2)] as const)
    line = points.map(([x, y], i) => `${i === 0 ? 'M' : 'L'}${x.toFixed(1)} ${y.toFixed(1)}`).join(' ')
    area = `${line} L${width} ${base} L0 ${base} Z`
  }

  return (
    <svg className="ml-spark" viewBox={`0 0 ${width} ${height}`} width={width} height={height} aria-hidden="true">
      <defs>
        <linearGradient id={fill} x1="0" y1="0" x2="0" y2="1">
          <stop offset="0%" style={{ stopColor: color, stopOpacity: 0.35 }} />
          <stop offset="100%" style={{ stopColor: color, stopOpacity: 0 }} />
        </linearGradient>
      </defs>
      {line ? (
        <>
          <path d={area} fill={`url(#${fill})`} />
          <path data-testid="spark-line" d={line} fill="none" style={{ stroke: color }} strokeWidth="1.5" strokeLinejoin="round" />
        </>
      ) : (
        <line x1="0" y1={base} x2={width} y2={base} stroke="var(--faint)" strokeDasharray="2 3" />
      )}
    </svg>
  )
}
