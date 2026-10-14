import { useLive } from './api/useLive'
import { Dashboard } from './links/Dashboard'

/** The page: the dashboard, fed by the live connection to the engine that served it. */
export default function App() {
  const live = useLive()
  return <Dashboard status={live.status} table={live.table} feed={live.feed} />
}
