import { useCallback, useEffect, useState } from 'react'

/** The page's two views. */
export type Tab = 'links' | 'downloads'

/** The tabs, in order, and their names on screen. */
export const TABS: readonly { tab: Tab; label: string }[] = [
  { tab: 'links', label: 'Links' },
  { tab: 'downloads', label: 'Downloads' },
]

/** The tab an address names: `#downloads` is Downloads, anything else Links. */
export function tabFrom(hash: string): Tab {
  return hash === '#downloads' ? 'downloads' : 'links'
}

/**
 * The tab shown, kept in the address as `#downloads`, so a reload, a
 * bookmark or a typed address opens it. Choosing a tab replaces the address
 * rather than adding to the history: the tabs are two views of one page. An
 * address changed any other way, typed or by Back, moves the tab too.
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
    window.history.replaceState(window.history.state, '', next === 'downloads' ? '#downloads' : pathname + search)
    setTab(next)
  }, [])

  return [tab, choose]
}
