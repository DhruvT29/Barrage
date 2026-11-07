import { useRef, useState } from 'react'

import type { Priority } from '../api/types'
import type { Asked } from '../overview/control'
import { RemoveIcon } from './icons'

import './downloads.css'

const PRIORITIES: { value: Priority; words: string }[] = [
  { value: 'high', words: 'High' },
  { value: 'normal', words: 'Normal' },
  { value: 'low', words: 'Low' },
]

type Props = {
  priority: Priority
  /** Whether it has ended: it then has no place in line to change. */
  ended: boolean
  /** Whether removing it would stop something running, so the card asks first. */
  running: boolean
  onPriority: (priority: Priority) => Promise<Asked>
  onRemove: () => Promise<Asked>
}

/**
 * A card's queue controls: its priority, while it can still change, and
 * Remove, in any state. Removing takes it off the list and leaves its files
 * alone. One running is stopped by that, so the card asks first; anything
 * else goes at once, since nothing is lost. A refusal is written under them
 * in the engine's words. The card itself changes when the engine's next
 * status says so.
 */
export function TransferControls({ priority, ended, running, onPriority, onRemove }: Props) {
  const [asking, setAsking] = useState(false)
  const [sending, setSending] = useState(false)
  const [refused, setRefused] = useState<string | null>(null)
  // A second press before the page redraws must not ask twice.
  const busy = useRef(false)

  async function send(ask: () => Promise<Asked>) {
    if (busy.current) return
    busy.current = true
    setSending(true)
    setRefused(null)
    const asked = await ask()
    busy.current = false
    setSending(false)
    if ('refused' in asked) setRefused(asked.refused)
  }

  function remove() {
    setAsking(false)
    void send(onRemove)
  }

  return (
    <div className="tq-controls">
      <div className="tq-control-row">
        {!ended && (
          <select
            className="tq-select"
            aria-label="Priority"
            value={priority}
            disabled={sending}
            onChange={(event) => void send(() => onPriority(event.target.value as Priority))}
          >
            {PRIORITIES.map((p) => (
              <option key={p.value} value={p.value}>
                {p.words}
              </option>
            ))}
          </select>
        )}
        <button
          type="button"
          className="tq-act tq-remove"
          aria-label="Remove"
          title="Remove from the list. Its files stay on disk."
          disabled={sending}
          onClick={() => (running ? setAsking(true) : remove())}
        >
          <RemoveIcon />
        </button>
      </div>
      {asking && (
        <div className="tq-confirm">
          <p>Stop it and remove it? Its files stay on disk.</p>
          <button type="button" className="tq-word danger" onClick={remove}>
            Stop and remove
          </button>
          <button type="button" className="tq-word" onClick={() => setAsking(false)}>
            Keep it
          </button>
        </div>
      )}
      {refused !== null && (
        <p className="tq-note failure" role="alert">
          {refused}
        </p>
      )}
    </div>
  )
}
