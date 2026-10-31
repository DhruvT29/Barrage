/** The ring's geometry: it opens on the right, starts below it and runs clockwise. */
const SIZE = 260
const C = SIZE / 2
const START = 50 // degrees clockwise from three o'clock
const SWEEP = 280
const R_TRACK = 102
const R_INNER = 86
const R_TICK = 124

/** One coloured stretch of the ring: a cable's share of the figure. */
export type GaugePart = { name: string; share: number; color: string }

/** A point on a circle of radius `r` at `deg` degrees clockwise from three o'clock. */
function at(r: number, deg: number): [number, number] {
  const rad = (deg * Math.PI) / 180
  return [C + r * Math.cos(rad), C + r * Math.sin(rad)]
}

/** The tick marks around the ring, every 5 degrees, longer every 35: the lit ones are under the filled arc. */
function Ticks({ filled }: { filled: number }) {
  const ticks = []
  for (let a = 0; a <= SWEEP; a += 5) {
    const major = a % 35 === 0
    const [x1, y1] = at(R_TICK, START + a)
    const [x2, y2] = at(R_TICK - (major ? 9 : 5), START + a)
    const lit = a <= filled * SWEEP && filled > 0
    ticks.push(
      <line
        key={a}
        x1={x1}
        y1={y1}
        x2={x2}
        y2={y2}
        className={`hero-tick${major ? ' major' : ''}${lit ? ' lit' : ''}`}
      />,
    )
  }
  return <g>{ticks}</g>
}

/**
 * The aggregate dial: a 280-degree ring filled to `filled` (0 to 1), the
 * filled arc split into each cable's colour by its share, tick marks
 * around it, and the figure in words in the middle. The ring is drawing
 * only; the figure, its unit and the caption are text.
 */
export function Gauge({
  filled,
  parts,
  figure,
  unit,
  caption,
}: {
  filled: number
  parts: GaugePart[]
  figure: string
  unit: string | null
  caption: string
}) {
  const fill = Math.max(0, Math.min(1, filled))
  const length = SWEEP * fill
  let offset = 0
  const arcs = parts.map((part) => {
    const arc = { ...part, from: offset, len: length * part.share }
    offset += arc.len
    return arc
  })

  return (
    <div className="hero-gauge">
      <svg viewBox={`0 0 ${SIZE} ${SIZE}`} aria-hidden="true" focusable="false">
        <defs>
          <radialGradient id="hero-gauge-core" cx="50%" cy="50%" r="50%">
            <stop offset="0%" stopColor="var(--green)" stopOpacity="0.09" />
            <stop offset="70%" stopColor="var(--green)" stopOpacity="0.02" />
            <stop offset="100%" stopColor="var(--green)" stopOpacity="0" />
          </radialGradient>
        </defs>
        <circle cx={C} cy={C} r={R_INNER - 6} fill="url(#hero-gauge-core)" />
        <Ticks filled={fill} />
        {/* The track: the whole sweep, unlit. */}
        <circle
          className="hero-ring-track"
          cx={C}
          cy={C}
          r={R_TRACK}
          pathLength={360}
          strokeDasharray={`${SWEEP} 360`}
          transform={`rotate(${START} ${C} ${C})`}
        />
        <circle
          className="hero-ring-inner"
          cx={C}
          cy={C}
          r={R_INNER}
          pathLength={360}
          strokeDasharray={`${SWEEP} 360`}
          transform={`rotate(${START} ${C} ${C})`}
        />
        <circle className="hero-ring-dots" cx={C} cy={C} r={R_INNER - 12} />
        {/* The filled arc, a stretch per cable in its colour. */}
        <g transform={`rotate(${START} ${C} ${C})`}>
          {arcs.map((arc) => (
            <circle
              key={arc.name}
              className="hero-ring-part"
              data-testid="gauge-part"
              cx={C}
              cy={C}
              r={R_TRACK}
              pathLength={360}
              style={{
                color: arc.color,
                strokeDasharray: `${arc.len} 360`,
                strokeDashoffset: -arc.from,
              }}
            />
          ))}
          {/* The end of the filled arc: a lit dot that travels with it. */}
          {fill > 0 && (
            <g className="hero-ring-head" style={{ transform: `rotate(${length}deg)` }}>
              <line x1={C + R_TRACK - 13} y1={C} x2={C + R_TRACK + 13} y2={C} className="hero-ring-needle" />
              <circle cx={C + R_TRACK} cy={C} r={5} className="hero-ring-dot" />
            </g>
          )}
        </g>
      </svg>
      <div className="hero-dial">
        <span className="hero-dial-label">Aggregate</span>
        <span className={unit ? 'hero-total' : 'hero-total unknown'} data-testid="total">
          <span className="hero-total-figure">{figure}</span>
          {unit && (
            <>
              {' '}
              <small>{unit}</small>
            </>
          )}
        </span>
        <span className="hero-dial-caption">{caption}</span>
      </div>
    </div>
  )
}
