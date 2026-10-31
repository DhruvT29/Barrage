import type { DownloadsView } from '../api/types'
import { type Cable, interfaceKind, liveBps, mbps, rtt } from './cables'
import { InterfaceIcon } from './icons'
import { count } from './messages'
import { nodeLabel, stateWords, tone } from './radarModel'

/** The states that carry data, as the engine's `LinkState::is_usable`. */
const usable = (c: Cable) => c.state === 'up' || c.state === 'degraded'

/**
 * Under the radar: the cables it shows as compact rows (icon, name, state in
 * words, measured speed and ping), each one a button that selects it, and
 * how many of all the cables cannot carry data right now.
 */
export function Roster({
  list,
  all,
  downloads,
  selected,
  onSelect,
  empty,
  known,
}: {
  /** The cables shown (the filter applied). */
  list: Cable[]
  /** Whether the engine has sent a table: before it, how many is unknown. */
  known: boolean
  /** Every cable, for the count of those not usable. */
  all: Cable[]
  downloads: DownloadsView | null
  selected: string | null
  onSelect: (name: string) => void
  empty: string | null
}) {
  const notUsable = all.filter((c) => !usable(c)).length

  return (
    <section className="rd-roster" aria-label="Contact roster">
      <div className="rd-sectionhead">
        <div>
          <p className="rd-eyebrow">Contact roster</p>
          <h2 className="rd-title">
            Detected <span className="rd-accent">{count(known ? list.length : null)}</span>
          </h2>
        </div>
        {all.length > 0 && (
          <span className="rd-aside">
            <span className={notUsable ? 'rd-pip rd-bg-down' : 'rd-pip rd-bg-up'} aria-hidden="true" />
            {notUsable ? `${notUsable} not usable` : 'all usable'}
          </span>
        )}
      </div>

      {empty ? (
        <p className="rd-quiet rd-rosterempty">{empty}</p>
      ) : (
        <div className="rd-rows">
          {list.map((cable) => {
            const carrying = liveBps(downloads, cable.name) !== null
            const t = tone(cable.state)
            return (
              <button
                key={cable.name}
                type="button"
                className={`rd-row rd-row-${t}`}
                aria-pressed={cable.name === selected}
                aria-label={nodeLabel(cable, carrying)}
                onClick={() => onSelect(cable.name)}
              >
                <span className={`rd-rowicon rd-tone-${t}`}>
                  <InterfaceIcon kind={interfaceKind(cable.name)} size={16} />
                </span>
                <span className="rd-rowid">
                  <span className="rd-rowname">{cable.name}</span>
                  <span className={`rd-rowstate rd-tone-${t}`}>{stateWords(cable.state, carrying)}</span>
                </span>
                <span className="rd-rowfigs">
                  <span className="rd-rowspeed">{cable.downBps === null ? '-' : mbps(cable.downBps)}</span>
                  <span className="rd-rowping">{rtt(cable.rttMs)}</span>
                </span>
              </button>
            )
          })}
        </div>
      )}
    </section>
  )
}
