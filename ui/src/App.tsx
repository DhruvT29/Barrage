import { useSpeedHistory } from './api/history'
import { useLive } from './api/useLive'
import { Dashboard } from './links/Dashboard'
import { useTab } from './tabs'

/** The page: the dashboard, fed by the live connection to the engine that served it. */
export default function App() {
  const live = useLive()
  const history = useSpeedHistory(live.table, live.downloads)
  const [tab, setTab] = useTab()
  return <Dashboard {...live} history={history} tab={tab} onTab={setTab} />
}
