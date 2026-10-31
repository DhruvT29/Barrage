import { useCallback, useEffect, useState } from 'react'

/** The page's two views: the download manager, and the radar of the cables. */
export type Tab = 'downloads' | 'links'

/** The tabs, in order, and their names on screen. */
export const TABS: readonly { tab: Tab; label: string }[] = [
  { tab: 'downloads', label: 'Download manager' },
  { tab: 'links', label: 'Device radar' },
]

/** The tab an address names: `#radar` (or the old `#links`) is the radar, anything else the download manager. */
export function tabFrom(hash: string): Tab {
  return hash === '#radar' || hash === '#links' ? 'links' : 'downloads'
}

/**
 * The tab shown, kept in the address as `#radar`, so a reload, a bookmark or
 * a typed address opens it. Choosing a tab replaces the address rather than
 * adding to the history: the tabs are two views of one page. An address
 * changed any other way, typed or by Back, moves the tab too.
 */
export function useTab(): [Tab, (tab: Tab) => void] {
  const [tab, setTab] = useState<Tab>(() => tabFrom(window.location.hash))

  useEffect(() => {
    const follow = () => setTab(tabFrom(window.location.hash))
    window.addEventListener('hashchange', follow)
    return () => window.removeEventListener('hashchange', follow)
  }, [])

  const choose = useCallback((next: Tab) => {
    const { pathname, search } = window.location
    window.history.replaceState(window.history.state, '', next === 'links' ? '#radar' : pathname + search)
    setTab(next)
  }, [])

  return [tab, choose]
}
