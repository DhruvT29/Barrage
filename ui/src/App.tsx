/** The page: a header bar with the name and the tabs. Only tabs for what the
 * engine really does appear: Links now, others as their phases arrive. */
export default function App() {
  return (
    <header className="bar">
      <h1 className="wordmark">BARRAGE</h1>
      <nav className="tabs" role="tablist" aria-label="Views">
        <button type="button" role="tab" aria-selected="true">
          Links
        </button>
      </nav>
    </header>
  )
}
