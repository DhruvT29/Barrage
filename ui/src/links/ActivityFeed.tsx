import './radar.css'

import type { ReactNode } from 'react'

import type { FeedLine } from '../api/useLive'

/** "13:22:41", this machine's local time. */
function clock(at: Date): string {
  return at.toTimeString().slice(0, 8)
}

/**
 * How a change line is marked: a link that went down, one that was added,
 * anything else. Read from the engine's own words, which are shown as they
 * are; the mark is only a tint beside them.
 */
function kind(text: string): 'down' | 'add' | 'info' {
  if (/-> down\b/.test(text) || text.startsWith('- ')) return 'down'
  if (text.startsWith('+ ')) return 'add'
  return 'info'
}

/**
 * The engine's words, exactly, with each "->" held together: a line may
 * wrap after a hyphen, and "up -" over "> down" would read as nonsense.
 */
function unbroken(text: string): ReactNode[] {
  return text.split('->').flatMap((part, i) =>
    i === 0
      ? [part]
      : [
          <span key={i} className="rd-nobreak">
            -&gt;
          </span>,
          part,
        ],
  )
}

/** The engine's change lines, newest first, as `useLive` keeps them: a log with each line's time. */
export function ActivityFeed({ feed }: { feed: FeedLine[] }) {
  return (
    <aside className="rd-panel rd-activity" aria-label="Activity">
      <div className="rd-sectionhead">
        <div>
          <p className="rd-eyebrow">Engine log</p>
          <h2 className="rd-title">Activity</h2>
        </div>
        <span className="rd-aside">{feed.length === 0 ? 'quiet' : `${feed.length} ${feed.length === 1 ? 'line' : 'lines'}`}</span>
      </div>
      <div className="rd-log">
        {feed.length === 0 ? (
          <p className="rd-quiet">Nothing has changed yet.</p>
        ) : (
          <ol>
            {feed.map((line, i) => (
              <li key={`${line.at.getTime()}-${i}-${line.text}`} className={`rd-log-${kind(line.text)}`}>
                <time dateTime={line.at.toISOString()}>{clock(line.at)}</time>
                <span>{unbroken(line.text)}</span>
              </li>
            ))}
          </ol>
        )}
        <p className="rd-prompt" aria-hidden="true">
          &gt; <span className="rd-cursor" />
        </p>
      </div>
    </aside>
  )
}
