import { type ReactNode, useEffect, useState } from 'react'
import {
  type BrowserOSPortSnapshot,
  type ResolvedPort,
  readBrowserOSPorts,
} from '@/modules/api/browseros-ports'
import { resolveCanonicalMcpEndpointUrl } from '@/modules/api/mcp-endpoint'

/**
 * `system/health` refreshes its link state on a one second poll server-side, so
 * a slower client poll is enough to notice an outage without extra load.
 */
const HEALTH_POLL_MS = 5000

type HealthState =
  | { status: 'loading' }
  | { status: 'unreachable' }
  | { status: 'ok'; connected: boolean; downForMs: number | null }

/**
 * The new tab page: deliberately blank. It carries only what cannot be checked
 * anywhere else without leaving the page — the MCP and CDP ports and whether
 * the browser link is alive — and one way back into the cockpit.
 *
 * Ports are labelled with where the number came from, because a fallback port
 * renders identically to a real one and would otherwise read as fact.
 */
export function BlankNewTab() {
  const [ports, setPorts] = useState<BrowserOSPortSnapshot | null>(null)
  const [mcpEndpoint, setMcpEndpoint] = useState<string | null>(null)
  const [health, setHealth] = useState<HealthState>({ status: 'loading' })

  useEffect(() => {
    let active = true

    void readBrowserOSPorts().then((snapshot) => {
      if (active) setPorts(snapshot)
    })
    void resolveCanonicalMcpEndpointUrl().then((url) => {
      if (active) setMcpEndpoint(url)
    })

    return () => {
      active = false
    }
  }, [])

  const serverPort = ports?.server.port

  useEffect(() => {
    if (!serverPort) return

    const controller = new AbortController()
    let active = true

    const read = async () => {
      try {
        const response = await fetch(
          `http://127.0.0.1:${serverPort}/system/health`,
          { signal: controller.signal },
        )
        if (!response.ok) throw new Error(`HTTP ${response.status}`)
        const body = (await response.json()) as {
          browser?: {
            connected?: boolean
            downForMs?: number | null
            lastError?: string | null
          }
        }
        if (!active) return
        setHealth({
          status: 'ok',
          connected: body.browser?.connected === true,
          downForMs: body.browser?.downForMs ?? null,
        })
      } catch {
        if (active) setHealth({ status: 'unreachable' })
      }
    }

    void read()
    const timer = setInterval(() => void read(), HEALTH_POLL_MS)

    return () => {
      active = false
      controller.abort()
      clearInterval(timer)
    }
  }, [serverPort])

  return (
    <div className="fixed inset-0 flex flex-col bg-white text-neutral-900">
      <main className="flex flex-1 items-center justify-center px-6">
        <div className="w-full max-w-md space-y-6">
          <p className="font-medium text-[11px] text-neutral-400 uppercase tracking-[0.18em]">
            NEOB
          </p>

          <dl className="space-y-3 text-sm">
            <Row label="MCP endpoint" value={mcpEndpoint ?? <Pending />} mono />
            <Row
              label="MCP port"
              value={portValue(ports?.proxy)}
              hint={sourceHint(ports?.proxy)}
            />
            <Row
              label="CDP port"
              value={portValue(ports?.cdp)}
              hint={ports?.cdp ? sourceHint(ports.cdp) : 'not published'}
            />
            <Row
              label="Browser link"
              value={<HealthValue health={health} />}
              hint={
                health.status === 'unreachable'
                  ? `127.0.0.1:${serverPort ?? '—'} unreachable`
                  : 'CDP link reported by the sidecar'
              }
            />
          </dl>
        </div>
      </main>

      <footer className="pb-8 text-center">
        <a
          className="text-neutral-400 text-xs transition-colors hover:text-neutral-700"
          href="#/cockpit"
        >
          open cockpit →
        </a>
      </footer>
    </div>
  )
}

function portValue(port: ResolvedPort | null | undefined) {
  if (!port) return <Pending />
  return <span className="font-mono tabular-nums">{port.port}</span>
}

function sourceHint(port: ResolvedPort | null | undefined) {
  if (!port) return undefined
  return port.source === 'pref'
    ? 'from browser pref'
    : 'fallback — not confirmed'
}

function HealthValue({ health }: { health: HealthState }) {
  if (health.status === 'loading') return <Pending />
  if (health.status === 'unreachable') return <span>disconnected</span>
  if (!health.connected) {
    const down =
      health.downForMs === null
        ? ''
        : ` · down ${Math.round(health.downForMs / 1000)}s`
    return <span>disconnected{down}</span>
  }
  return <span>connected</span>
}

function Pending() {
  return <span className="text-neutral-300">—</span>
}

function Row({
  label,
  value,
  hint,
  mono = false,
}: {
  label: string
  value: ReactNode
  hint?: string
  mono?: boolean
}) {
  return (
    <div className="flex gap-4">
      <dt className="w-32 shrink-0 pt-0.5 text-neutral-400">{label}</dt>
      <dd className="min-w-0">
        <div
          className={
            mono
              ? 'break-all font-mono text-[13px] leading-5'
              : 'text-neutral-900 leading-5'
          }
        >
          {value}
        </div>
        {hint ? (
          <div className="pt-0.5 text-[11px] text-neutral-400 leading-4">
            {hint}
          </div>
        ) : null}
      </dd>
    </div>
  )
}
