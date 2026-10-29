import type { DownloadsView } from '../api/types'
import { AddForm } from './AddForm'
import { DownloadCard } from './DownloadCard'

/**
 * The Downloads tab: the field to add a link, then every download since the
 * engine started, the newest first, two to a row on a wide screen. The field
 * stays put while the list scrolls under it.
 */
export function Downloads({ downloads }: { downloads: DownloadsView | null }) {
  // The engine lists them in the order they were added.
  const newestFirst = downloads ? [...downloads.list].reverse() : []

  return (
    <main className="downloads">
      <AddForm />
      <div className="list">
        {downloads !== null && newestFirst.length === 0 && (
          <p className="quiet wide">Nothing downloaded yet. Paste a link to a file above.</p>
        )}
        {newestFirst.map((d) => (
          <DownloadCard key={d.id} download={d} />
        ))}
      </div>
    </main>
  )
}
