// The preview page (preview.html): the real Dashboard fed made-up messages
// that change every second, to look at the design without an engine. Only
// `pnpm dev` serves it; the engine's page is built from index.html alone.
//
//   /preview.html                 everything running
//   /preview.html#radar           the radar
//   /preview.html?empty           an engine with no cables and no downloads
//   /preview.html?disconnected    the engine stopped answering (the last table dimmed)
//   /preview.html?connecting      before the first table
//   /preview.html?long            the longest words a card can be given
import { StrictMode, useEffect, useState } from 'react'
import { createRoot } from 'react-dom/client'

import '../fonts'
import '../app.css'

import { useSpeedHistory } from '../api/history'
import type { DownloadsView, TableView } from '../api/types'
import type { FeedLine, Status } from '../api/useLive'
import { Dashboard } from '../links/Dashboard'
import { useTab } from '../tabs'
import { PREVIEW_FEED, previewDownloads, previewTable, withLongWords } from './fixtures'

const query = new URLSearchParams(window.location.search)

// eslint-disable-next-line react-refresh/only-export-components -- the entry point, never hot-swapped
function Preview() {
  const [t, setT] = useState(0)
  useEffect(() => {
    const timer = setInterval(() => setT((n) => n + 1), 1000)
    return () => clearInterval(timer)
  }, [])

  const status: Status = query.has('disconnected') ? 'disconnected' : query.has('connecting') ? 'connecting' : 'live'
  let table: TableView | null = previewTable(t)
  let downloads: DownloadsView | null = previewDownloads(t)
  if (query.has('long')) [table, downloads] = withLongWords(table, downloads)
  if (query.has('empty')) {
    table = { links: [], shared: [], connections: [], combined: { bps: 0, parts: [], not_counted: [] } }
    downloads = { live: { bps: 0, parts: [] }, list: [], used: [] }
  }
  if (status === 'connecting') {
    table = null
    downloads = null
  }
  const feed: FeedLine[] = PREVIEW_FEED.map((text, i) => ({ at: new Date(Date.now() - i * 47_000), text }))
  const history = useSpeedHistory(table, downloads)
  const [tab, setTab] = useTab()

  return (
    <Dashboard status={status} table={table} feed={feed} downloads={downloads} history={history} tab={tab} onTab={setTab} />
  )
}

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <Preview />
  </StrictMode>,
)
