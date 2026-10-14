import type { FeedLine } from '../api/useLive'

/** "13:22:41", this machine's local time. */
function clock(at: Date): string {
  return at.toTimeString().slice(0, 8)
}

/** The engine's change lines, newest first, as `useLive` keeps them. */
export function ActivityFeed({ feed }: { feed: FeedLine[] }) {
  return (
    <aside className="activity" aria-label="Activity">
      <h2>Activity</h2>
      {feed.length === 0 ? (
        <p className="quiet">Nothing has changed yet.</p>
      ) : (
        <ol>
          {feed.map((line, i) => (
            <li key={`${line.at.getTime()}-${i}-${line.text}`}>
              <time dateTime={line.at.toISOString()}>{clock(line.at)}</time>
              <span>{line.text}</span>
            </li>
          ))}
        </ol>
      )}
    </aside>
  )
}
