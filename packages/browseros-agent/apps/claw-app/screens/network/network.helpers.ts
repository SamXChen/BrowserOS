/**
 * Pure helpers for the Network screen. Everything here is render-free so the
 * list, the detail panes and the rule cards share one implementation.
 */

import type {
  NetworkBodyPage,
  NetworkRecord,
  NetworkRule,
  NetworkRuleAction,
  NetworkRuleRevision,
} from '@browseros/claw-api'
import { networkCopy } from './network.copy'

export type RecordFacet =
  | 'all'
  | '2xx'
  | '3xx'
  | '4xx'
  | '5xx'
  | 'failed'
  | 'pending'

export const STATUS_FACETS: RecordFacet[] = [
  'all',
  '2xx',
  '3xx',
  '4xx',
  '5xx',
  'failed',
  'pending',
]

export const METHODS = ['GET', 'POST', 'PUT', 'PATCH', 'DELETE', 'OPTIONS']

export interface RecordFilters {
  query: string
  method: string
  status: RecordFacet
  resourceType: string
  tab: string
  mock: boolean
  body: boolean
  hideAgent: boolean
}

export const DEFAULT_FILTERS: RecordFilters = {
  query: '',
  method: 'all',
  status: 'all',
  resourceType: 'all',
  tab: 'all',
  mock: false,
  body: false,
  hideAgent: true,
}

export interface RecordDerived {
  host: string
  path: string
  collector: boolean
}

export type DerivedRecord = NetworkRecord & { derived?: RecordDerived }

function parseUrl(url: string): URL | null {
  try {
    return new URL(url)
  } catch {
    return null
  }
}

/** The Network panel's own tab and its API calls never belong in the list. */
export function isCollectorRequest(record: NetworkRecord): boolean {
  if (record.tabTitle === 'NEOB') return true
  const url = parseUrl(record.url)
  if (!url) return false
  return (
    url.pathname.startsWith('/api/v1/network') ||
    url.pathname === '/system/health'
  )
}

/** Pre-computed once per record, so filtering never parses URLs again. */
export function recordDerived(record: NetworkRecord): RecordDerived {
  const url = parseUrl(record.url)
  return {
    host: url?.origin ?? '',
    path: url ? `${url.pathname}${url.search}` : record.url,
    collector: isCollectorRequest(record),
  }
}

export function matchesStatus(
  record: NetworkRecord,
  facet: RecordFacet,
): boolean {
  if (facet === 'all') return true
  if (facet === 'failed') return record.state === 'failed'
  if (facet === 'pending') {
    return record.state !== 'failed' && record.status === undefined
  }
  if (record.status === undefined) return false
  return `${Math.floor(record.status / 100)}xx` === facet
}

/** The single status-code palette. */
export function statusTone(status?: number, failed = false): string {
  if (failed || (status !== undefined && status >= 500)) {
    return 'text-red-600 dark:text-red-400'
  }
  if (status === undefined) return 'text-ink-3'
  if (status >= 400) return 'text-amber-600 dark:text-amber-400'
  if (status >= 300) return 'text-ink-2'
  if (status >= 200) return 'text-emerald-600 dark:text-emerald-400'
  return 'text-ink-3'
}

export function methodTone(method: string): string {
  switch (method.toUpperCase()) {
    case 'GET':
      return 'bg-cyanotype-blue/10 text-cyanotype-blue'
    case 'POST':
      return 'bg-emerald-500/10 text-emerald-700 dark:text-emerald-400'
    case 'PUT':
    case 'PATCH':
      return 'bg-amber-500/15 text-amber-700 dark:text-amber-400'
    case 'DELETE':
      return 'bg-red-500/10 text-red-600 dark:text-red-400'
    default:
      return 'bg-card-tint text-ink-2'
  }
}

export function formatTime(ms: number): string {
  return new Date(ms).toLocaleTimeString([], {
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
  })
}

export function formatRelative(ms: number, now = Date.now()): string {
  const diff = Math.max(0, now - ms)
  if (diff < 5_000) return 'now'
  if (diff < 60_000) return `${Math.floor(diff / 1000)}s`
  if (diff < 3_600_000) return `${Math.floor(diff / 60_000)}m`
  if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)}h`
  return formatTime(ms)
}

export function formatDuration(ms: number): string {
  if (ms < 1000) return `${Math.max(1, Math.round(ms))}ms`
  return `${(ms / 1000).toFixed(1)}s`
}

export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
}

export function prettify(value: string): string {
  const trimmed = value.trim()
  if (!trimmed.startsWith('{') && !trimmed.startsWith('[')) return value
  try {
    return JSON.stringify(JSON.parse(trimmed), null, 2)
  } catch {
    return value
  }
}

export function filterRecords(
  records: DerivedRecord[],
  filters: RecordFilters,
): DerivedRecord[] {
  const tokens = filters.query.trim().toLowerCase().split(/\s+/).filter(Boolean)
  return records.filter((record) => {
    const derived = record.derived ?? recordDerived(record)
    if (filters.hideAgent && derived.collector) return false
    if (
      filters.method !== 'all' &&
      record.method.toUpperCase() !== filters.method
    ) {
      return false
    }
    if (!matchesStatus(record, filters.status)) return false
    if (
      filters.resourceType !== 'all' &&
      (record.resourceType ?? '') !== filters.resourceType
    ) {
      return false
    }
    if (filters.tab !== 'all' && (record.tabTitle ?? '') !== filters.tab) {
      return false
    }
    if (filters.mock && !record.mock) return false
    if (filters.body && record.bodyState !== 'available') return false
    if (tokens.length === 0) return true
    const haystack = `${record.method} ${record.url} ${record.tabTitle ?? ''} ${
      record.resourceType ?? ''
    }`.toLowerCase()
    return tokens.every((token) => haystack.includes(token))
  })
}

/**
 * Decodes the loaded body pages. Pages are independent base64 chunks, so each
 * one is decoded on its own: decoding the concatenation breaks padding that
 * lands in the middle.
 */
export function decodeBase64Pages(
  pages: Array<Pick<NetworkBodyPage, 'encoding' | 'data'>>,
): Uint8Array {
  const chunks: Uint8Array[] = []
  let total = 0
  for (const page of pages) {
    let bytes: Uint8Array
    if (page.encoding === 'base64') {
      const binary = atob(page.data)
      bytes = new Uint8Array(binary.length)
      for (let index = 0; index < binary.length; index += 1) {
        bytes[index] = binary.charCodeAt(index)
      }
    } else {
      bytes = new TextEncoder().encode(page.data)
    }
    chunks.push(bytes)
    total += bytes.length
  }
  const joined = new Uint8Array(total)
  let offset = 0
  for (const chunk of chunks) {
    joined.set(chunk, offset)
    offset += chunk.length
  }
  return joined
}

export interface RuleStateView {
  label: string
  tone: string
  dot: string
}

const STATE_TONES = {
  active: {
    tone: 'bg-cyanotype-live text-cyanotype-live-ink',
    dot: 'bg-cyanotype-live-ink',
  },
  pending: {
    tone: 'bg-amber-500/15 text-amber-700 dark:text-amber-400',
    dot: 'bg-amber-500',
  },
  attention: {
    tone: 'bg-red-500/10 text-red-600 dark:text-red-400',
    dot: 'bg-red-500',
  },
  disabled: {
    tone: 'bg-card-tint text-ink-2',
    dot: 'bg-ink-3',
  },
} as const

export function ruleStateView(
  rule: NetworkRule,
  resolveName?: (id: string) => string | undefined,
): RuleStateView {
  const states = networkCopy.states
  switch (rule.state) {
    case 'active':
      return { label: states.active, ...STATE_TONES.active }
    case 'pending': {
      const label =
        rule.stateReason === 'no_tab'
          ? states.pendingNoTab
          : rule.stateReason === 'apply_timeout'
            ? states.pendingTimeout
            : states.pendingNoBrowser
      return { label, ...STATE_TONES.pending }
    }
    case 'unbound':
      return { label: states.unbound, ...STATE_TONES.attention }
    case 'failed': {
      const reason = rule.stateReason
        ? states.failedReason[rule.stateReason]
        : undefined
      return {
        label: reason ? `${states.failed} · ${reason}` : states.failed,
        ...STATE_TONES.attention,
      }
    }
    default: {
      const reason = rule.stateReason ?? ''
      if (reason.startsWith('replaced_by:')) {
        const id = reason.slice('replaced_by:'.length)
        return {
          label: networkCopy.rules.hint.replaced(resolveName?.(id) ?? id),
          ...STATE_TONES.disabled,
        }
      }
      if (reason === 'stopped_all') {
        return { label: states.disabledStoppedAll, ...STATE_TONES.disabled }
      }
      if (reason === 'superseded') {
        return { label: states.superseded, ...STATE_TONES.disabled }
      }
      return { label: states.disabled, ...STATE_TONES.disabled }
    }
  }
}

const STATE_RANK: Record<string, number> = {
  active: 0,
  pending: 1,
  unbound: 2,
  failed: 3,
  disabled: 4,
}

export function sortRules<T extends NetworkRule>(rules: T[]): T[] {
  return rules.slice().sort((left, right) => {
    const rank = (STATE_RANK[left.state] ?? 5) - (STATE_RANK[right.state] ?? 5)
    if (rank !== 0) return rank
    return (right.updatedAt ?? 0) - (left.updatedAt ?? 0)
  })
}

function sameBody(
  left: NetworkRuleAction['body'],
  right: NetworkRuleAction['body'],
): boolean {
  return (left?.bytes ?? 0) === (right?.bytes ?? 0)
}

/** Field-level change list between two revisions. */
export function diffRevisions(
  previous: NetworkRuleRevision,
  current: NetworkRuleRevision,
): string[] {
  const labels = networkCopy.rules.detail
  const changes: string[] = []
  const left = previous.matcher
  const right = current.matcher
  const matcherChanged =
    left.method !== right.method ||
    left.origin !== right.origin ||
    left.pathname !== right.pathname ||
    left.exactUrl !== right.exactUrl ||
    left.resourceType !== right.resourceType ||
    JSON.stringify(left.sampleQuery ?? []) !==
      JSON.stringify(right.sampleQuery ?? [])
  if (matcherChanged) changes.push(labels.matcher)
  if (
    JSON.stringify(left.queryPolicy?.ignoreKeys ?? []) !==
    JSON.stringify(right.queryPolicy?.ignoreKeys ?? [])
  ) {
    changes.push('ignoreKeys')
  }
  if (left.queryPolicy?.mode !== right.queryPolicy?.mode) {
    changes.push('Query 模式')
  }
  if (
    JSON.stringify(left.bodyFields ?? []) !==
    JSON.stringify(right.bodyFields ?? [])
  ) {
    changes.push('bodyFields')
  }
  const leftAction = previous.action
  const rightAction = current.action
  if (leftAction.kind !== rightAction.kind) changes.push('类型')
  if (leftAction.status !== rightAction.status) changes.push('响应状态')
  if (
    JSON.stringify(leftAction.headers ?? null) !==
    JSON.stringify(rightAction.headers ?? null)
  ) {
    changes.push('响应 headers')
  }
  if (!sameBody(leftAction.body, rightAction.body)) changes.push('响应 body')
  if ((leftAction.script ?? '') !== (rightAction.script ?? '')) {
    changes.push('处理器脚本')
  }
  return changes
}
