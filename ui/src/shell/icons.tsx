// The shell's and the hero's icons: simple 1.5px strokes in currentColor,
// drawn on a 16-unit grid. Decorative only: the words beside them carry the
// meaning, so every icon is hidden from assistive technology.
import type { ReactNode } from 'react'

function Icon({ children, size = 16 }: { children: ReactNode; size?: number }) {
  return (
    <svg
      className="icon"
      width={size}
      height={size}
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.5"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      focusable="false"
    >
      {children}
    </svg>
  )
}

/** A cloud with an arrow down: the download manager. */
export const DownloadIcon = () => (
  <Icon>
    <path d="M4.5 11.5H4a3 3 0 0 1-.4-5.97A4 4 0 0 1 11.4 4.6 3.2 3.2 0 0 1 12 11.5h-.5" />
    <path d="M8 7.5v6M5.8 11.3 8 13.5l2.2-2.2" />
  </Icon>
)

/** Rings and a sweep: the radar. */
export const RadarIcon = () => (
  <Icon>
    <circle cx="8" cy="8" r="6" />
    <circle cx="8" cy="8" r="3" />
    <path d="M8 8l4.2-4.2" />
    <circle cx="8" cy="8" r="0.6" fill="currentColor" />
  </Icon>
)

/** A heartbeat line: the connection to the engine. */
export const PulseIcon = () => (
  <Icon>
    <path d="M1 8.5h3l1.6-4 2.8 8 1.8-5.5.9 1.5H15" />
  </Icon>
)

export const PlusIcon = () => (
  <Icon>
    <path d="M8 3v10M3 8h10" />
  </Icon>
)

export const StopIcon = () => (
  <Icon>
    <rect x="4" y="4" width="8" height="8" rx="1" />
  </Icon>
)

export const CloseIcon = () => (
  <Icon>
    <path d="M4 4l8 8M12 4l-8 8" />
  </Icon>
)

export const ArrowIcon = () => (
  <Icon>
    <path d="M3 8h10M9 4l4 4-4 4" />
  </Icon>
)

/** Two chain links: an address to download. */
export const LinkIcon = () => (
  <Icon>
    <path d="M6.5 9.5l3-3" />
    <path d="M7 4.6l1-1a2.8 2.8 0 0 1 4 4l-1 1" />
    <path d="M9 11.4l-1 1a2.8 2.8 0 0 1-4-4l1-1" />
  </Icon>
)

/** Three joined nodes: cables linked. */
export const NodesIcon = () => (
  <Icon>
    <circle cx="4" cy="4" r="1.8" />
    <circle cx="12" cy="5" r="1.8" />
    <circle cx="7" cy="12" r="1.8" />
    <path d="M5.7 4.4l4.5.4M4.8 5.6l1.4 4.7M10.8 6.4l-2.6 4.1" />
  </Icon>
)

/** A stopwatch: the round trip. */
export const PingIcon = () => (
  <Icon>
    <circle cx="8" cy="9" r="5" />
    <path d="M8 9V6.5M6.5 2h3M12 4.5l1-1" />
  </Icon>
)

/** A mast sending: downloads active. */
export const ActiveIcon = () => (
  <Icon>
    <circle cx="8" cy="8" r="1.2" />
    <path d="M5.2 5.2a4 4 0 0 0 0 5.6M10.8 5.2a4 4 0 0 1 0 5.6M3.1 3.1a7 7 0 0 0 0 9.8M12.9 3.1a7 7 0 0 1 0 9.8" />
  </Icon>
)

/** A tray with an arrow in: bytes fetched. */
export const FetchedIcon = () => (
  <Icon>
    <path d="M8 2v7.5M5 6.8 8 9.8l3-3" />
    <path d="M2.5 10v2.5a1 1 0 0 0 1 1h9a1 1 0 0 0 1-1V10" />
  </Icon>
)
