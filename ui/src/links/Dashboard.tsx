import { useMemo, useState } from 'react'

import { EMPTY_HISTORY, type SpeedHistory } from '../api/history'
import type { FeedLine, Status } from '../api/useLive'
import type { DownloadsView, TableView } from '../api/types'
import { TransferQueue } from '../downloads/TransferQueue'
import { AddModal } from '../overview/AddModal'
import { Hero } from '../overview/Hero'
import { colorsByName } from '../palette'
import { Header } from '../shell/Header'
import type { Tab } from '../tabs'
import { MeshLinks } from './MeshLinks'
import { RadarView } from './Radar'

/** The props every section of the page is drawn from. */
export type PageProps = {
  status: Status
  table: TableView | null
  feed: FeedLine[]
  downloads: DownloadsView | null
  history: SpeedHistory
  /** Each cable's colour by name, in the table's order (palette.ts). */
  colors: Record<string, string>
}

/**
 * The whole page, from what `useLive` holds, the speed history and the tab
 * shown: the header with the tabs and the top figures, then either the
 * download manager (the hero, the cables beside the transfer queue) or the
 * radar of the cables. The same props always draw the same page; the only
 * state of its own is whether the add-a-download window is open.
 */
export function Dashboard({
  status,
  table,
  feed,
  downloads,
  history = EMPTY_HISTORY,
  tab,
  onTab,
}: Omit<PageProps, 'colors' | 'history'> & { history?: SpeedHistory; tab: Tab; onTab: (tab: Tab) => void }) {
  const [adding, setAdding] = useState(false)
  const colors = useMemo(() => {
    const names: string[] = []
    for (const link of table?.links ?? []) if (!names.includes(link.name)) names.push(link.name)
    return colorsByName(names)
  }, [table])

  // Disconnected with a table: the last one stays, dimmed, so it reads as old.
  const stale = status === 'disconnected' && table !== null
  const page: PageProps = { status, table, feed, downloads, history, colors }

  return (
    <>
      <Header {...page} tab={tab} onTab={onTab} />
      <main className={stale ? 'page stale' : 'page'}>
        {tab === 'downloads' ? (
          <>
            <Hero {...page} onAdd={() => setAdding(true)} onTab={onTab} />
            <div className="manager-grid">
              <MeshLinks {...page} />
              <TransferQueue {...page} onAdd={() => setAdding(true)} />
            </div>
          </>
        ) : (
          <RadarView {...page} />
        )}
      </main>
      <AddModal open={adding} onClose={() => setAdding(false)} />
    </>
  )
}
