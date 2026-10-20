/**
 * Minimal typed client for the network capture REST surface.
 *
 * The generated `claw-api-client` cannot carry these paths until the OpenAPI
 * contract is regenerated, so this module keeps the wire calls in one place
 * instead of scattering `fetch` across the screen.
 */

import { resolveApiBaseUrl } from './client'

export interface HeaderPair {
  name: string
  value: string
}

export interface MockMark {
  ruleId: string
  revision: number
  outcome: string
  reason?: string
}

export interface NetworkRecord {
  recordId: string
  seq: number
  requestId: string
  redirectOrdinal: number
  tabId?: number
  tabTitle?: string
  pageId?: number
  frameId?: string
  method: string
  url: string
  resourceType?: string
  requestHeaders?: HeaderPair[]
  postData?: string
  status?: number
  statusText?: string
  mimeType?: string
  responseHeaders?: HeaderPair[]
  encodedDataLength?: number
  startedAtMs: number
  finishedAtMs?: number
  fromCache?: boolean
  fromServiceWorker?: boolean
  state: string
  failure?: string
  bodyState: string
  bodyReason?: string
  mock?: MockMark
}

export interface NetworkRule {
  id: string
  scope: 'all_tabs' | 'source_tab'
  scopeTab?: number | null
  enabled: boolean
  state: string
  stateReason?: string
  authorClient?: string | null
  currentRevision: number
  method?: string
  pathname?: string
  origin?: string
  ignoreKeys?: string[]
  strictQuery?: boolean
  responseStatus?: number
  createdAt: number
  updatedAt: number
  lastHit?: {
    at: number
    outcome: string
    url: string
    method: string
    tabTitle?: string
  } | null
}

export interface NearMiss {
  recordId?: string
  method?: string
  url?: string
  reason: string
  queryDiffs?: Array<{
    key: string
    sample: string[]
    actual: string[]
  }>
  suggestion?: { ignoreKeys: string[] }
}

export interface NetworkCaptureState {
  running: boolean
  state?: 'running' | 'paused' | 'stopped'

  startedAt?: number | null
  records: number
  gapEvents: number
  bodyBytes?: number
  tabs?: Array<{
    tabId: number
    pageId: number
    title: string
    url: string
    attached: boolean
  }>
}

export interface RuleRevision {
  revision: number
  matcher: {
    method?: string
    origin?: string
    pathname?: string
    queryPolicy?: { mode?: 'lenient' | 'strict'; ignoreKeys?: string[] }
    exactUrl?: string | null
    bodyFields?: unknown[]
  }
  response: {
    status?: number
    headers?: Record<string, string | null>
    body?: { kind?: string; text?: string }
    delayMs?: number
  }
  createdAt: number
  source?: { handoffId?: string | null; recordId?: string | null }
}

export interface RuleDetail {
  rule: NetworkRule
  revision: RuleRevision | null
  bodyPreview?: { encoding: 'utf8' | 'base64'; text: string } | null
  hitCount: number
  hits: Array<{
    id: string
    outcome: string
    at: number
    url: string
    method: string
    reason?: string | null
  }>
}

export interface NetworkStatus {
  browser: { connected: boolean }
  capture: NetworkCaptureState
  rules: { active: number; total: number }
  runtime?: {
    paused: number
    decisionP95Ms: number
    rulesGeneration: number
  }
}

export interface BodyPage {
  encoding: 'utf8' | 'base64'
  offset: number
  total: number
  truncated: boolean
  data: string
  state?: string
  mimeType?: string
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const base = await resolveApiBaseUrl()
  const response = await fetch(`${base}/api/v1/network${path}`, {
    ...init,
    headers: {
      ...(init?.body ? { 'content-type': 'application/json' } : {}),
      ...init?.headers,
    },
  })
  if (!response.ok) {
    const body = await response.text()
    throw new Error(body || `${response.status} ${response.statusText}`)
  }
  if (response.status === 204) return undefined as T
  return (await response.json()) as T
}

export const networkApi = {
  state: () => request<NetworkStatus>('/state'),
  startCapture: () =>
    request<NetworkCaptureState>('/captures', { method: 'POST', body: '{}' }),
  stopCapture: () =>
    request<NetworkCaptureState>('/captures', { method: 'DELETE' }),
  records: (limit = 500) =>
    request<{ records: NetworkRecord[] }>(`/records?limit=${limit}`),
  responseBody: (recordId: string, limit = 65536, offset = 0) =>
    request<BodyPage>(
      `/records/${encodeURIComponent(recordId)}/response-body?limit=${limit}&offset=${offset}`,
    ),
  requestBody: (recordId: string, limit = 65536, offset = 0) =>
    request<BodyPage>(
      `/records/${encodeURIComponent(recordId)}/request-body?limit=${limit}&offset=${offset}`,
    ),
  deleteRecord: (recordId: string) =>
    request<{ deleted: boolean; undoToken: string; undoUntil: number }>(
      `/records/${encodeURIComponent(recordId)}`,
      { method: 'DELETE' },
    ),
  restoreRecord: (undoToken: string) =>
    request<{ restored: boolean }>('/records/restore', {
      method: 'POST',
      body: JSON.stringify({ undoToken }),
    }),
  deleteRecords: () =>
    request<{ deleted: number }>('/records', { method: 'DELETE' }),
  createHandoff: (recordId: string) =>
    request<{ handoffId: string; clipboardText: string; expiresAt: number }>(
      '/handoffs',
      {
        method: 'POST',
        body: JSON.stringify({ recordId }),
      },
    ),
  rules: () => request<{ rules: NetworkRule[] }>('/rules'),
  setRuleEnabled: (ruleId: string, enabled: boolean) =>
    request<{ ruleId: string; enabled: boolean; state: string }>(
      `/rules/${encodeURIComponent(ruleId)}`,
      { method: 'PATCH', body: JSON.stringify({ enabled }) },
    ),
  ruleDetail: (ruleId: string) =>
    request<RuleDetail>(`/rules/${encodeURIComponent(ruleId)}`),
  ruleRevisions: (ruleId: string) =>
    request<{ revisions: RuleRevision[] }>(
      `/rules/${encodeURIComponent(ruleId)}/revisions`,
    ),
  restoreRuleRevision: (ruleId: string, revision: number) =>
    request<{ revision: number; state: string }>(
      `/rules/${encodeURIComponent(ruleId)}/revisions/${revision}/restore`,
      { method: 'POST' },
    ),
  testRule: (ruleId: string) =>
    request<{
      matched: number
      considered: number
      samples: Array<{
        recordId: string
        method: string
        url: string
        status?: number
        tabTitle?: string
      }>
    }>(`/rules/${encodeURIComponent(ruleId)}/test`, { method: 'POST' }),
  setRuleScope: (ruleId: string, scope: 'all_tabs' | 'tab', tabId?: number) =>
    request<{
      ruleId: string
      scope: string
      scopeTab?: number
      state: string
    }>(`/rules/${encodeURIComponent(ruleId)}`, {
      method: 'PATCH',
      body: JSON.stringify({ scope, tabId }),
    }),
  deleteRule: (ruleId: string) =>
    request<{ deleted: boolean; undoToken: string; undoUntil: number }>(
      `/rules/${encodeURIComponent(ruleId)}`,
      { method: 'DELETE' },
    ),
  restoreRule: (ruleId: string) =>
    request<{ restored: boolean; state: string }>(
      `/rules/${encodeURIComponent(ruleId)}/restore`,
      { method: 'POST' },
    ),
  stopAll: () =>
    request<{ disabled: number; narrowed: boolean }>('/stop-all', {
      method: 'POST',
    }),
  nearMisses: (ruleId: string) =>
    request<{ nearMisses: NearMiss[] }>(
      `/rules/${encodeURIComponent(ruleId)}/near-misses`,
    ),
  ignoreKeys: (ruleId: string, keys: string[]) =>
    request<{
      ruleId: string
      revision: number
      ignoreKeys: string[]
      state: string
    }>(`/rules/${encodeURIComponent(ruleId)}/ignore-keys`, {
      method: 'POST',
      body: JSON.stringify({ keys }),
    }),
  eventUrl: async () => `${await resolveApiBaseUrl()}/api/v1/network/events`,
}
