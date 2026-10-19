/// <reference path="./chrome-browser-os.d.ts" />

import { CLAW_API_PORT_DEFAULT } from '@browseros/shared/constants/ports'
import {
  API_URL_STORAGE_KEY,
  type ApiBaseUrlSources,
  normalizeLoopbackApiRootUrl,
  resolveApiBaseUrlFromSources,
} from './client.helpers'

const BROWSEROS_SERVER_PORT_PREF = 'browseros.server.server_port'
const BROWSEROS_PROXY_PORT_PREF = 'browseros.server.proxy_port'
const BROWSEROS_CDP_PORT_PREF = 'browseros.server.cdp_port'

/** A port together with where the number came from, so UI can label it. */
export interface ResolvedPort {
  port: number
  source: 'pref' | 'fallback'
}

export interface BrowserOSPortSnapshot {
  /** Sidecar HTTP API port. */
  server: ResolvedPort
  /** MCP proxy port, the one the `/mcp` endpoint is served on. */
  proxy: ResolvedPort
  /**
   * CDP port. `null` when the browser never published it; the launcher must
   * pass `--browseros-cdp-port` for the pref to carry the real value.
   */
  cdp: ResolvedPort | null
}

export function fallbackClawApiBaseUrl(): string {
  return `http://127.0.0.1:${CLAW_API_PORT_DEFAULT}`
}

/** Builds trusted fallback sources from the current extension window. */
export function apiBaseUrlSourcesFromWindow(
  fallback = fallbackClawApiBaseUrl(),
): ApiBaseUrlSources {
  if (typeof window === 'undefined') {
    return {
      query: null,
      stored: null,
      launcher: null,
      fallback,
    }
  }

  const query = new URLSearchParams(window.location.search).get('apiUrl')
  const queryBaseUrl = normalizeLoopbackApiRootUrl(query)
  if (queryBaseUrl) {
    try {
      window.sessionStorage.setItem(API_URL_STORAGE_KEY, queryBaseUrl)
    } catch {}
  }

  let stored: string | null = null
  try {
    stored = window.sessionStorage.getItem(API_URL_STORAGE_KEY)
  } catch {}

  return {
    query,
    stored,
    launcher: import.meta.env.VITE_BROWSEROS_CLAW_API_URL,
    fallback,
  }
}

/** Resolves the BrowserOS-managed sidecar server base URL. */
export async function resolveBrowserOSServerBaseUrl(
  sources = apiBaseUrlSourcesFromWindow(),
): Promise<string> {
  const port = await readBrowserOSPort(BROWSEROS_SERVER_PORT_PREF)
  return port ? loopbackBaseUrl(port) : resolveApiBaseUrlFromSources(sources)
}

/** Resolves the BrowserOS-managed MCP proxy base URL. */
export async function resolveBrowserOSMcpBaseUrl(
  sources = apiBaseUrlSourcesFromWindow(),
): Promise<string> {
  const port = await readBrowserOSPort(BROWSEROS_PROXY_PORT_PREF)
  return port ? loopbackBaseUrl(port) : resolveApiBaseUrlFromSources(sources)
}

/**
 * Reads every port the new-tab panel shows, keeping each value's provenance so
 * the UI can say whether it is real or a fallback.
 *
 * Server and proxy fall back to the same trusted sources the API client uses,
 * because that is where traffic actually goes. CDP has no honest fallback:
 * printing a guess would show a port nothing listens on, so it stays `null`
 * and the panel reports it as unknown.
 */
export async function readBrowserOSPorts(
  sources = apiBaseUrlSourcesFromWindow(),
): Promise<BrowserOSPortSnapshot> {
  const [serverPort, proxyPort, cdpPort] = await Promise.all([
    readBrowserOSPort(BROWSEROS_SERVER_PORT_PREF),
    readBrowserOSPort(BROWSEROS_PROXY_PORT_PREF),
    readBrowserOSPort(BROWSEROS_CDP_PORT_PREF),
  ])

  const fallbackPort = portFromBaseUrl(resolveApiBaseUrlFromSources(sources))

  return {
    server: serverPort
      ? { port: serverPort, source: 'pref' }
      : { port: fallbackPort, source: 'fallback' },
    proxy: proxyPort
      ? { port: proxyPort, source: 'pref' }
      : { port: fallbackPort, source: 'fallback' },
    cdp: cdpPort ? { port: cdpPort, source: 'pref' } : null,
  }
}

function portFromBaseUrl(baseUrl: string): number {
  try {
    return validPort(Number(new URL(baseUrl).port)) ?? CLAW_API_PORT_DEFAULT
  } catch {
    return CLAW_API_PORT_DEFAULT
  }
}

async function readBrowserOSPort(prefName: string): Promise<number | null> {
  if (
    typeof chrome === 'undefined' ||
    typeof chrome.browserOS?.getPref !== 'function'
  ) {
    return null
  }

  try {
    const pref = await new Promise<chrome.browserOS.PrefObject>(
      (resolve, reject) => {
        chrome.browserOS.getPref(prefName, (value) => {
          const message = chrome.runtime?.lastError?.message
          if (message) {
            reject(new Error(message))
            return
          }
          resolve(value)
        })
      },
    )
    return validPort(pref.value)
  } catch {
    return null
  }
}

function validPort(value: unknown): number | null {
  if (typeof value !== 'number') return null
  return Number.isInteger(value) && value > 0 && value <= 65535 ? value : null
}

function loopbackBaseUrl(port: number): string {
  return `http://127.0.0.1:${port}`
}
