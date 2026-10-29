import { useLive } from './api/useLive'
import { Dashboard } from './links/Dashboard'
import { useTab } from './tabs'

/** The page: the dashboard, fed by the live connection to the engine that served it. */
export default function App() {
  const live = useLive()
  const [tab, setTab] = useTab()
  return <Dashboard {...live} tab={tab} onTab={setTab} />
}
