// Messages exactly as the engine sends them, written by its test
// `the_ui_contract_file_is_up_to_date` (engine/src/api/view.rs). Do not
// edit: `satisfies` holds them to types.ts whenever the page is
// type-checked (`pnpm build`), and contract.test.tsx renders them. After
// changing the engine's JSON, set BARRAGE_WRITE_CONTRACT=1 and run, from
// engine/: cargo test --lib the_ui_contract_file
import type { LiveMessage } from './types'

export const messages = [
  {
    "type": "table",
    "table": {
      "links": [
        {
          "id": 1,
          "name": "Wi-Fi",
          "address": "192.0.2.10",
          "family": "ipv4",
          "state": "up",
          "rtt_ms": 40,
          "down_bps": 57600000,
          "public_ip": "203.0.113.7"
        },
        {
          "id": 2,
          "name": "Ethernet 2",
          "address": "198.51.100.20",
          "family": "ipv4",
          "state": "up",
          "rtt_ms": 120,
          "down_bps": 141000000,
          "public_ip": "203.0.113.50"
        },
        {
          "id": 4,
          "name": "Ethernet 3",
          "address": "198.51.100.30",
          "family": "ipv4",
          "state": "up",
          "rtt_ms": 48,
          "down_bps": null,
          "public_ip": "203.0.113.7"
        },
        {
          "id": 5,
          "name": "Ethernet 4",
          "address": "198.51.100.40",
          "family": "ipv4",
          "state": "degraded",
          "rtt_ms": 300,
          "down_bps": 9400000,
          "public_ip": "198.51.100.40"
        },
        {
          "id": 6,
          "name": "Ethernet 4",
          "address": "2001:db8::40",
          "family": "ipv6",
          "state": "portal",
          "rtt_ms": 60,
          "down_bps": null,
          "public_ip": null
        },
        {
          "id": 7,
          "name": "Ethernet 5",
          "address": "192.0.2.50",
          "family": "ipv4",
          "state": "up",
          "rtt_ms": null,
          "down_bps": null,
          "public_ip": "203.0.113.90"
        },
        {
          "id": 8,
          "name": "Ethernet 6",
          "address": "192.0.2.60",
          "family": "ipv4",
          "state": "portal",
          "rtt_ms": 80,
          "down_bps": null,
          "public_ip": null
        },
        {
          "id": 9,
          "name": "Ethernet 7",
          "address": "192.0.2.70",
          "family": "ipv4",
          "state": "probing",
          "rtt_ms": null,
          "down_bps": null,
          "public_ip": null
        },
        {
          "id": 10,
          "name": "Ethernet 8",
          "address": "192.0.2.80",
          "family": "ipv4",
          "state": "down",
          "rtt_ms": null,
          "down_bps": null,
          "public_ip": null
        }
      ],
      "shared": [
        [
          1,
          4
        ]
      ],
      "connections": [
        [
          "Wi-Fi",
          "Ethernet 3"
        ]
      ],
      "combined": {
        "bps": 208000000,
        "parts": [
          {
            "name": "Wi-Fi",
            "bps": 57600000,
            "shared_with": []
          },
          {
            "name": "Ethernet 2",
            "bps": 141000000,
            "shared_with": []
          },
          {
            "name": "Ethernet 4",
            "bps": 9400000,
            "shared_with": []
          }
        ],
        "not_counted": [
          {
            "name": "Ethernet 3",
            "why": "not measured yet"
          },
          {
            "name": "Ethernet 5",
            "why": "not measured yet"
          },
          {
            "name": "Ethernet 6",
            "why": "portal"
          },
          {
            "name": "Ethernet 7",
            "why": "probing"
          },
          {
            "name": "Ethernet 8",
            "why": "down"
          }
        ]
      }
    }
  },
  {
    "type": "downloads",
    "downloads": {
      "live": {
        "bps": 126900000,
        "parts": [
          {
            "name": "Wi-Fi",
            "bps": 38400000
          },
          {
            "name": "Ethernet 2",
            "bps": 88500000
          }
        ]
      },
      "list": [
        {
          "id": 1,
          "url": "https://example.com/files/ubuntu.iso",
          "name": "ubuntu.iso",
          "state": "downloading",
          "length": 6114656256,
          "done": 2527000000,
          "bps": 126900000,
          "note": null,
          "cables": [
            {
              "name": "Wi-Fi",
              "bytes": 1210000000,
              "bps": 38400000,
              "connections": 4,
              "notes": []
            },
            {
              "name": "Ethernet 2",
              "bytes": 1317000000,
              "bps": 88500000,
              "connections": 8,
              "notes": []
            },
            {
              "name": "Ethernet 3",
              "bytes": 0,
              "bps": 0,
              "connections": 0,
              "notes": [
                "sits out: the server gave this cable another version of the file (\"7b1d\", not \"a3f0\")"
              ]
            }
          ]
        },
        {
          "id": 2,
          "url": "https://example.com/files/photo.jpg",
          "name": "photo (1).jpg",
          "state": "done",
          "length": 2400000,
          "done": 2400000,
          "bps": 0,
          "note": null,
          "cables": [
            {
              "name": "Wi-Fi",
              "bytes": 2400000,
              "bps": 0,
              "connections": 0,
              "notes": []
            }
          ]
        },
        {
          "id": 3,
          "url": "https://example.com/files/gone.zip",
          "name": null,
          "state": "failed",
          "length": null,
          "done": 0,
          "bps": 0,
          "note": "finding https://example.com/files/gone.zip through 192.0.2.10: https://example.com/files/gone.zip answered 404 Not Found",
          "cables": [
            {
              "name": "Wi-Fi",
              "bytes": 0,
              "bps": 0,
              "connections": 0,
              "notes": [
                "finding https://example.com/files/gone.zip through 192.0.2.10: https://example.com/files/gone.zip answered 404 Not Found"
              ]
            }
          ]
        },
        {
          "id": 4,
          "url": "https://example.com/files/notes.pdf",
          "name": null,
          "state": "waiting",
          "length": null,
          "done": 0,
          "bps": 0,
          "note": null,
          "cables": []
        },
        {
          "id": 5,
          "url": "https://example.com/files/video.mp4",
          "name": null,
          "state": "starting",
          "length": null,
          "done": 0,
          "bps": 0,
          "note": null,
          "cables": []
        }
      ]
    }
  },
  {
    "type": "change",
    "table": {
      "links": [
        {
          "id": 1,
          "name": "Wi-Fi",
          "address": "192.0.2.10",
          "family": "ipv4",
          "state": "up",
          "rtt_ms": 40,
          "down_bps": 57600000,
          "public_ip": "203.0.113.7"
        },
        {
          "id": 2,
          "name": "Ethernet 2",
          "address": "198.51.100.20",
          "family": "ipv4",
          "state": "up",
          "rtt_ms": 120,
          "down_bps": 141000000,
          "public_ip": "203.0.113.50"
        },
        {
          "id": 3,
          "name": "Ethernet 2",
          "address": "2001:db8::20",
          "family": "ipv6",
          "state": "up",
          "rtt_ms": 95,
          "down_bps": 141000000,
          "public_ip": "2001:db8::20"
        },
        {
          "id": 4,
          "name": "Ethernet 3",
          "address": "198.51.100.30",
          "family": "ipv4",
          "state": "up",
          "rtt_ms": 48,
          "down_bps": 41000000,
          "public_ip": "203.0.113.7"
        },
        {
          "id": 5,
          "name": "Ethernet 4",
          "address": "198.51.100.40",
          "family": "ipv4",
          "state": "degraded",
          "rtt_ms": 300,
          "down_bps": 9400000,
          "public_ip": "198.51.100.40"
        },
        {
          "id": 6,
          "name": "Ethernet 4",
          "address": "2001:db8::40",
          "family": "ipv6",
          "state": "portal",
          "rtt_ms": 60,
          "down_bps": null,
          "public_ip": null
        },
        {
          "id": 7,
          "name": "Ethernet 5",
          "address": "192.0.2.50",
          "family": "ipv4",
          "state": "up",
          "rtt_ms": null,
          "down_bps": null,
          "public_ip": "203.0.113.90"
        },
        {
          "id": 8,
          "name": "Ethernet 6",
          "address": "192.0.2.60",
          "family": "ipv4",
          "state": "portal",
          "rtt_ms": 80,
          "down_bps": null,
          "public_ip": null
        },
        {
          "id": 9,
          "name": "Ethernet 7",
          "address": "192.0.2.70",
          "family": "ipv4",
          "state": "probing",
          "rtt_ms": null,
          "down_bps": null,
          "public_ip": null
        },
        {
          "id": 10,
          "name": "Ethernet 8",
          "address": "192.0.2.80",
          "family": "ipv4",
          "state": "down",
          "rtt_ms": null,
          "down_bps": null,
          "public_ip": null
        }
      ],
      "shared": [
        [
          1,
          4
        ]
      ],
      "connections": [
        [
          "Wi-Fi",
          "Ethernet 3"
        ]
      ],
      "combined": {
        "bps": 208000000,
        "parts": [
          {
            "name": "Wi-Fi",
            "bps": 57600000,
            "shared_with": [
              "Ethernet 3"
            ]
          },
          {
            "name": "Ethernet 2",
            "bps": 141000000,
            "shared_with": []
          },
          {
            "name": "Ethernet 4",
            "bps": 9400000,
            "shared_with": []
          }
        ],
        "not_counted": [
          {
            "name": "Ethernet 5",
            "why": "not measured yet"
          },
          {
            "name": "Ethernet 6",
            "why": "portal"
          },
          {
            "name": "Ethernet 7",
            "why": "probing"
          },
          {
            "name": "Ethernet 8",
            "why": "down"
          }
        ]
      }
    },
    "lines": [
      "appeared: Ethernet 2 2001:db8::20 up, rtt 95 ms, speed 141.0 Mbps, public 2001:db8::20",
      "Ethernet 3 198.51.100.30: speed 41.0 Mbps"
    ]
  }
] satisfies LiveMessage[]
