import { useRef, useState } from 'react'

import type { Asked } from '../overview/control'

/**
 * The card's button: whether an answer is awaited, and the engine's last
 * refusal, shown only while the card is in the state it was refused in. The
 * card itself changes when the engine's next status says so.
 */
export function useAction<State>(
  id: number,
  state: State,
  action: 'stop' | 'resume' | null,
  ask: (id: number, action: 'stop' | 'resume') => Promise<Asked>,
) {
  const [refused, setRefused] = useState<{ words: string; state: State } | null>(null)
  const [sending, setSending] = useState(false)
  // A second press before the page redraws must not ask twice.
  const busy = useRef(false)

  async function press() {
    if (action === null || busy.current) return
    busy.current = true
    setSending(true)
    setRefused(null)
    const asked = await ask(id, action)
    busy.current = false
    setSending(false)
    if ('refused' in asked) setRefused({ words: asked.refused, state })
  }

  return { action, sending, refused: refused?.state === state ? refused.words : null, press }
}
