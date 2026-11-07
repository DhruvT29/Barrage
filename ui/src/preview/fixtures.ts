// Made-up engine messages for the preview page (preview.html), never built
// into the engine's page. Addresses are documentation ranges only
// (192.0.2.x, 198.51.100.x, 203.0.113.x, 2001:db8::): the repository is public.
import type { DownloadsView, DownloadView, LinkView, QueueView, TableView, TorrentsView, TorrentView } from '../api/types'

const MB = 8e6

function wiggle(base: number, t: number, phase: number): number {
  return Math.max(0, Math.round(base * (1 + 0.18 * Math.sin(t / 3 + phase) + 0.07 * Math.sin(t * 1.7 + phase * 2))))
}

/**
 * The table at second `t`: Wi-Fi, an Ethernet adapter, a tethered phone with
 * two addresses, and a dead adapter. The phone goes degraded for the first 8
 * seconds of every 40, both addresses, so the whole cable does and comes back.
 */
export function previewTable(t: number): TableView {
  const links: LinkView[] = [
    { id: 1, name: 'Wi-Fi', address: '192.0.2.24', family: 'ipv4', state: 'up', rtt_ms: 12, down_bps: 18 * MB, public_ip: '203.0.113.7', metered: false },
    { id: 2, name: 'Ethernet', address: '192.0.2.81', family: 'ipv4', state: 'up', rtt_ms: 4, down_bps: 30 * MB, public_ip: '203.0.113.9', metered: false },
    { id: 3, name: 'Ethernet 2', address: '198.51.100.12', family: 'ipv4', state: t % 40 < 8 ? 'degraded' : 'up', rtt_ms: 38, down_bps: 9 * MB, public_ip: '203.0.113.44', metered: true },
    { id: 4, name: 'Ethernet 2', address: '2001:db8::5', family: 'ipv6', state: t % 40 < 8 ? 'degraded' : 'up', rtt_ms: 41, down_bps: 9 * MB, public_ip: '2001:db8::5', metered: true },
    { id: 5, name: 'Bluetooth Network', address: '192.0.2.200', family: 'ipv4', state: 'down', rtt_ms: null, down_bps: null, public_ip: null, metered: false },
  ]
  const parts = [
    { name: 'Ethernet', bps: 30 * MB, shared_with: [] },
    { name: 'Wi-Fi', bps: 18 * MB, shared_with: [] },
    { name: 'Ethernet 2', bps: 9 * MB, shared_with: [] },
  ]
  return {
    links,
    shared: [],
    connections: [['Wi-Fi'], ['Ethernet'], ['Ethernet 2']],
    combined: { bps: parts.reduce((a, p) => a + p.bps, 0), parts, not_counted: [{ name: 'Bluetooth Network', why: 'down' }] },
  }
}

/** The queue beside the downloads and torrents below: two in line, one of each kind. */
export function previewQueue(notice: boolean): QueueView {
  return {
    limit: 3,
    most: 20,
    holding: 3,
    places: [
      { kind: 'web', id: 90, place: 1 },
      { kind: 'torrent', id: 91, place: 2 },
    ],
    notice: notice ? 'The saved list could not be read, so the engine started with an empty one. It was kept as state.broken-1700000000.json.' : null,
    used_since: 1_700_000_000,
  }
}

/** The downloads at second `t`: two downloading, one stopped, one done, one failed, one waiting. */
export function previewDownloads(t: number): DownloadsView {
  const wifi = wiggle(11 * MB, t, 0)
  const eth = wiggle(22 * MB, t, 1.3)
  const phone = wiggle(6 * MB, t, 2.6)
  const big = 34.2e9
  const bigDone = Math.min(big, 16.6e9 + (t * (wifi + eth)) / 8 / 2)
  const iso = 4.7e9
  const isoDone = Math.min(iso, 1.1e9 + (t * phone) / 8)

  const list: DownloadView[] = [
    {
      priority: 'high',
      id: 90, url: 'https://releases.example.com/fedora.iso', name: null,
      state: 'queued', length: null, done: 0, bps: 0, note: null, cables: [],
    },
    {
      priority: 'normal',
      id: 1, url: 'https://releases.example.com/linux/distro-24.04-desktop-amd64.iso', name: 'distro-24.04-desktop-amd64.iso',
      state: 'done', length: 5.8e9, done: 5.8e9, bps: 0, note: null,
      cables: [
        { name: 'Ethernet', bytes: 3.9e9, bps: 0, connections: 0, wanted: 0, notes: [] },
        { name: 'Wi-Fi', bytes: 1.9e9, bps: 0, connections: 0, wanted: 0, notes: [] },
      ],
    },
    {
      priority: 'normal',
      id: 2, url: 'https://mirror.example.org/archive/OpenData_Spectra_Archive_2026.tar.gz', name: 'OpenData_Spectra_Archive_2026.tar.gz',
      state: 'downloading', length: big, done: bigDone, bps: wifi + eth,
      note: null,
      cables: [
        { name: 'Ethernet', bytes: bigDone * 0.62, bps: eth, connections: 9, wanted: 9, notes: [] },
        { name: 'Wi-Fi', bytes: bigDone * 0.38, bps: wifi, connections: 5, wanted: 6, notes: [] },
      ],
    },
    {
      priority: 'normal',
      id: 3, url: 'https://cdn.example.net/builds/game-patch-v2.12.iso', name: 'game-patch-v2.12.iso',
      state: 'downloading', length: iso, done: isoDone, bps: phone, note: null,
      cables: [
        { name: 'Ethernet 2', bytes: isoDone, bps: phone, connections: t % 40 < 8 ? 1 : 2, wanted: 2, notes: t % 40 < 8 ? ['degraded: holding one connection until trusted'] : [] },
      ],
    },
    {
      priority: 'normal',
      id: 4, url: 'https://downloads.example.com/netinst-amd64.iso', name: 'netinst-amd64.iso',
      state: 'stopped', length: 1.9e9, done: 1.4e9, bps: 0, note: null,
      cables: [{ name: 'Wi-Fi', bytes: 1.4e9, bps: 0, connections: 0, wanted: 0, notes: [] }],
    },
    {
      priority: 'normal',
      id: 5, url: 'https://files.example.org/missing/dataset.zip', name: 'dataset.zip',
      state: 'failed', length: null, done: 0, bps: 0, note: 'the server answered 404 Not Found',
      cables: [],
    },
    {
      priority: 'normal',
      id: 6, url: 'https://slow.example.com/firmware.bin', name: null,
      state: 'waiting', length: null, done: 0, bps: 0, note: null, cables: [],
    },
  ]
  const parts = [
    { name: 'Ethernet', bps: eth },
    { name: 'Wi-Fi', bps: wifi },
    { name: 'Ethernet 2', bps: phone },
  ]
  return {
    live: { bps: wifi + eth + phone, parts },
    list,
    used: [
      { cable: 'Ethernet', bytes: 3.9e9 + bigDone * 0.62, sent: 0 },
      { cable: 'Wi-Fi', bytes: 3.3e9 + bigDone * 0.38, sent: 0 },
      { cable: 'Ethernet 2', bytes: isoDone, sent: 0 },
    ],
  }
}

/** A few of the engine's change lines, for the activity feed. */
export const PREVIEW_FEED = [
  'Ethernet 2 198.51.100.12: degraded -> up',
  'Ethernet 2 198.51.100.12: up -> degraded',
  'Bluetooth Network 192.0.2.200: up -> down',
  'Wi-Fi 192.0.2.24: probing -> up, 12 ms',
]

/**
 * The `?long` preview: the same page with the longest words a card can be
 * given, to see that nothing runs out of its card: a long interface name with
 * long IPv6 addresses, a file name with no break in it, and an address with
 * no name yet.
 */
export function withLongWords(table: TableView, downloads: DownloadsView): [TableView, DownloadsView] {
  const name = 'Remote NDIS based Internet Sharing Device #2 (USB tethering adapter)'
  const links: LinkView[] = [
    ...table.links,
    { id: 90, name, address: '2001:db8:85a3:1234:5678:8a2e:370:7334', family: 'ipv6', state: 'up', rtt_ms: 96, down_bps: 4 * MB, public_ip: '2001:db8:85a3:ffff:ffff:ffff:ffff:fffe', metered: true },
    { id: 91, name, address: '2001:db8:85a3:1234:abcd:ef01:2345:6789', family: 'ipv6', state: 'probing', rtt_ms: null, down_bps: null, public_ip: null, metered: true },
  ]
  const long: DownloadView[] = [
    {
      priority: 'normal',
      id: 7, url: 'https://very-long-subdomain-name-for-a-mirror.downloads.example.com/a/b/c/d/Supercalifragilistic_Expialidocious_Dataset_Export_2026-10-31T23-59-59_part-0001-of-0042.tar.zst',
      name: 'Supercalifragilistic_Expialidocious_Dataset_Export_2026-10-31T23-59-59_part-0001-of-0042.tar.zst',
      state: 'downloading', length: 912e9, done: 3.3e9, bps: 3 * MB, note: null,
      cables: [{ name, bytes: 3.3e9, bps: 3 * MB, connections: 1, wanted: 4, notes: ['new address: holding one connection until it proves itself on this download'] }],
    },
    {
      priority: 'normal',
      id: 8, url: 'https://slow.example.com/firmware/builds/nightly/2026/10/31/device-firmware-image-without-any-spaces-at-all-0123456789abcdef0123456789abcdef.bin',
      name: null, state: 'starting', length: null, done: 0, bps: 0, note: null, cables: [],
    },
  ]
  return [
    { ...table, links },
    {
      ...downloads,
      live: { bps: downloads.live.bps + 3 * MB, parts: [...downloads.live.parts, { name, bps: 3 * MB }] },
      list: [...downloads.list, ...long],
      used: [...downloads.used, { cable: name, bytes: 3.3e9, sent: 0 }],
    },
  ]
}

/** The torrents at second `t`: one in each phase, one downloading through two cables, one of them metered. */
export function previewTorrents(t: number): TorrentsView {
  const wifi = wiggle(9 * MB, t, 0.7)
  const phone = wiggle(5 * MB, t, 3.1)
  const whole = 3.2e9
  const fromWifi = Math.min(whole * 0.7, 1.1e9 + (t * wifi) / 8)
  const fromPhone = Math.min(whole * 0.3, 0.42e9 + (t * phone) / 8)
  const done = Math.floor(fromWifi + fromPhone)
  const list: TorrentView[] = [
    {
      priority: 'low',
      id: 91, name: 'archlinux.iso', info_hash: 'b'.repeat(40), phase: 'queued',
      length: null, done: 0, down_bps: 0, up_bps: 0, peers: 0, note: null, cables: [],
    },
    {
      priority: 'normal',
      id: 1, name: 'a-distribution-netinst-amd64.iso', info_hash: 'a'.repeat(40), phase: 'done', length: 660e6, done: 660e6,
      down_bps: 0, up_bps: 0, peers: 0, note: null,
      cables: [{ name: 'Wi-Fi', received: 660e6, sent: 41e6, bps: 0, connections: 0, uploads: true }],
    },
    {
      priority: 'normal',
      id: 2, name: 'open-movie-2160p.mkv', info_hash: 'b'.repeat(40), phase: 'stopped', length: 8.4e9, done: 2.9e9,
      down_bps: 0, up_bps: 0, peers: 0, note: null,
      cables: [
        { name: 'Wi-Fi', received: 2.2e9, sent: 180e6, bps: 0, connections: 0, uploads: true },
        { name: 'Ethernet 2', received: 0.7e9, sent: 0, bps: 0, connections: 0, uploads: false },
      ],
    },
    {
      priority: 'normal',
      id: 3, name: 'research-dataset-2026.tar', info_hash: 'c'.repeat(40), phase: 'failed', length: 21e9, done: 4e9,
      down_bps: 0, up_bps: 0, peers: 0, note: 'error writing to disk: there is not enough space on the disk', cables: [],
    },
    {
      priority: 'normal',
      id: 4, name: 'distro-24.04-desktop-amd64.iso', info_hash: 'd'.repeat(40), phase: 'downloading', length: whole, done,
      down_bps: wifi + phone, up_bps: wiggle(1.4 * MB, t, 1.9), peers: 23, note: null,
      cables: [
        { name: 'Wi-Fi', received: Math.floor(fromWifi), sent: 96e6, bps: wifi, connections: 16, uploads: true },
        { name: 'Ethernet 2', received: Math.floor(fromPhone), sent: 0, bps: phone, connections: 7, uploads: false },
      ],
    },
    {
      priority: 'normal',
      id: 5, name: null, info_hash: '0123456789abcdef0123456789abcdef01234567', phase: 'finding', length: null, done: 0,
      down_bps: 0, up_bps: 0, peers: 0, note: null, cables: [],
    },
  ]
  return {
    list,
    udp_cable: 'Wi-Fi',
    listening: t % 40 < 20
      ? { state: 'trying', cable: 'Wi-Fi', why: null }
      : {
          state: 'closed',
          cable: 'Wi-Fi',
          why: 'no peer called in within 10 minutes; this PC cannot be reached from outside on Wi-Fi',
        },
  }
}
