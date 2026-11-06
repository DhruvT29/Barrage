import { useState } from 'react'

import type { QueueView } from '../api/types'
import { resetUsed, setLimit, type Asked } from '../overview/control'

import './downloads.css'

/**
 * The queue's own line, above its cards: how many run of how many may, the
 * limit, and since when the data used has been counting, with a way to zero
 * it that asks first. A refusal is written under it in the engine's words.
 */
export function QueueBar({ queue }: { queue: QueueView }) {
  const [asking, setAsking] = useState(false)
  const [refused, setRefused] = useState<string | null>(null)

  async function send(ask: () => Promise<Asked>) {
    setRefused(null)
    const asked = await ask()
    if ('refused' in asked) setRefused(asked.refused)
  }

  const limits = Array.from({ length: queue.most }, (_unused, i) => i + 1)
  return (
    <div className="tq-bar">
      <div className="tq-bar-row">
        <span className="tq-running" data-testid="running">
          {queue.holding} of {queue.limit} running
        </span>
        <label className="tq-limit">
          <span>Run at once</span>
          <select
            className="tq-select"
            value={queue.limit}
            onChange={(event) => void send(() => setLimit(Number(event.target.value)))}
          >
            {limits.map((n) => (
              <option key={n} value={n}>
                {n}
              </option>
            ))}
          </select>
        </label>
        <span className="tq-used" data-testid="used-since">
          Data used since {new Date(queue.used_since * 1000).toLocaleDateString()}
          {!asking && (
            <button type="button" className="tq-word" onClick={() => setAsking(true)}>
              Reset
            </button>
          )}
          {asking && (
            <>
              <button
                type="button"
                className="tq-word danger"
                onClick={() => {
                  setAsking(false)
                  void send(resetUsed)
                }}
              >
                Reset to zero
              </button>
              <button type="button" className="tq-word" onClick={() => setAsking(false)}>
                Cancel
              </button>
            </>
          )}
        </span>
      </div>
      {refused !== null && (
        <p className="tq-note failure" role="alert">
          {refused}
        </p>
      )}
    </div>
  )
}

/** What the engine has to say about the saved list, until the user dismisses it. A new notice shows again. */
export function QueueNotice({ notice }: { notice: string | null }) {
  const [dismissed, setDismissed] = useState<string | null>(null)
  if (notice === null || notice === dismissed) return null
  return (
    <div className="tq-notice" role="status">
      <p>{notice}</p>
      <button type="button" className="tq-word" onClick={() => setDismissed(notice)}>
        Dismiss
      </button>
    </div>
  )
}
