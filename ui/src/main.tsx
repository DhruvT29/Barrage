import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'

// Bundled with the page, never fetched from the internet: the page is
// watched exactly when a connection drops.
import '@fontsource/oxanium/600.css'
import '@fontsource/oxanium/700.css'
import '@fontsource/oxanium/800.css'
import '@fontsource/saira/400.css'
import '@fontsource/saira/500.css'
import '@fontsource/saira/600.css'
import './app.css'

import App from './App'

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <App />
  </StrictMode>,
)
