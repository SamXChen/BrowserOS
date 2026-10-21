/**
 * Composes the Network queries, the SSE stream and the records store, and maps
 * events into the query cache.
 */

import type {
  NetworkCaptureState,
  NetworkRule,
  NetworkRuleList,
  NetworkStatus,
} from '@browseros/claw-api'
import type { QueryClient } from '@tanstack/react-query'
import { useQueryClient } from '@tanstack/react-query'
import {
  useCallback,
  useEffect,
  useRef,
  useState,
  useSyncExternalStore,
} from 'react'
import {
  type ConnectionState,
  connectNetworkEvents,
  type NetworkEvent,
} from '@/modules/api/network.events'
import {
  useCreateNetworkHandoff,
  useDeleteAllNetworkRules,
  useDeleteNetworkRecord,
  useDeleteNetworkRecords,
  useDeleteNetworkRule,
  useNetworkRecords,
  useNetworkRules,
  useNetworkStatus,
  usePatchNetworkRule,
  useReloadNetworkTab,
  useRestoreNetworkRecord,
  useRestoreNetworkRule,
  useRestoreNetworkRuleRevision,
  useStartNetworkCapture,
  useStopAllNetworkMocks,
  useStopNetworkCapture,
  useTestNetworkRule,
} from '@/modules/api/network.hooks'
import { type DerivedRecord, sortRules } from './network.helpers'
import { createRecordsStore } from './network.records-store'

export const RECORD_RENDER_LIMIT = 500

export const STATUS_KEY = ['api', 'network', 'status'] as const
export const RULES_KEY = ['api', 'network', 'rules'] as const
export const ruleKey = (id: string) =>
  ['api', 'network', 'rule', { id }] as const
export const ruleRevisionsKey = (id: string) =>
  ['api', 'network', 'rule-revisions', { id }] as const

function upsertRule(rules: NetworkRule[], next: NetworkRule): NetworkRule[] {
  const index = rules.findIndex((rule) => rule.id === next.id)
  if (index === -1) return [next, ...rules]
  const copy = rules.slice()
  copy[index] = next
  return copy
}

const detailTimers = new Map<string, ReturnType<typeof setTimeout>>()

function scheduleDetailRefresh(queryClient: QueryClient, ruleId: string): void {
  const pending = detailTimers.get(ruleId)
  if (pending) clearTimeout(pending)
  detailTimers.set(
    ruleId,
    setTimeout(() => {
      detailTimers.delete(ruleId)
      void queryClient.invalidateQueries({ queryKey: ruleKey(ruleId) })
      void queryClient.invalidateQueries({ queryKey: ruleRevisionsKey(ruleId) })
    }, 500),
  )
}

/** Pure cache mapping; exported so tests can drive it without React. */
export function applyNetworkEvent(
  queryClient: QueryClient,
  store: ReturnType<typeof createRecordsStore>,
  event: NetworkEvent,
): void {
  switch (event.type) {
    case 'snapshot': {
      const incoming = event.data
      queryClient.setQueryData<NetworkStatus>(STATUS_KEY, (existing) => ({
        ...incoming,
        runtime: incoming.runtime ?? existing?.runtime,
      }))
      return
    }
    case 'capture': {
      const capture = event.data as NetworkCaptureState
      queryClient.setQueryData<NetworkStatus>(STATUS_KEY, (existing) =>
        existing ? { ...existing, capture } : existing,
      )
      return
    }
    case 'rule': {
      const next = event.data
      queryClient.setQueryData<NetworkRuleList>(RULES_KEY, (existing) => ({
        rules: upsertRule(existing?.rules ?? [], next),
      }))
      const detail = queryClient.getQueryData<{ rule: NetworkRule }>(
        ruleKey(next.id),
      )
      const cached = detail?.rule
      if (
        cached &&
        (cached.currentRevision !== next.currentRevision ||
          cached.updatedAt !== next.updatedAt)
      ) {
        void queryClient.invalidateQueries({ queryKey: ruleKey(next.id) })
        void queryClient.invalidateQueries({
          queryKey: ruleRevisionsKey(next.id),
        })
      }
      return
    }
    case 'deleted': {
      if (event.data?.kind !== 'rule') {
        store.enqueue(event)
        return
      }
      const id = event.data.id
      if (!id) return
      queryClient.setQueryData<NetworkRuleList>(RULES_KEY, (existing) => ({
        rules: (existing?.rules ?? []).filter((rule) => rule.id !== id),
      }))
      queryClient.removeQueries({ queryKey: ruleKey(id) })
      queryClient.removeQueries({ queryKey: ruleRevisionsKey(id) })
      return
    }
    case 'hit': {
      const hit = event.data
      const counted = hit.outcome === 'fulfilled' || hit.outcome === 'processed'
      queryClient.setQueryData<NetworkRuleList>(RULES_KEY, (existing) => {
        if (!existing) return existing
        return {
          rules: existing.rules.map((rule) =>
            rule.id === hit.ruleId
              ? {
                  ...rule,
                  lastHit: hit,
                  hitCount: (rule.hitCount ?? 0) + (counted ? 1 : 0),
                }
              : rule,
          ),
        }
      })
      scheduleDetailRefresh(queryClient, hit.ruleId)
      return
    }
    case 'gap': {
      if (event.data?.lost) {
        queryClient.setQueryData<NetworkStatus>(STATUS_KEY, (existing) =>
          existing
            ? {
                ...existing,
                capture: {
                  ...existing.capture,
                  gapEvents:
                    existing.capture.gapEvents + (event.data.lost ?? 0),
                },
              }
            : existing,
        )
      }
      return
    }
    default:
      store.enqueue(event)
  }
}

/** Only the rule being toggled shows a pending state. */
export function isRulePending(
  mutation: { isPending?: boolean; variables?: { id?: string } },
  id: string,
): boolean {
  return Boolean(mutation.isPending && mutation.variables?.id === id)
}

/** The response body loads only when the Response tab is open. */
export function shouldLoadResponseBody(
  tab: string,
  bodyState: string | undefined,
): boolean {
  return tab === 'response' && bodyState === 'available'
}

export function toggleRuleOptimistic(
  queryClient: QueryClient,
  ruleId: string,
  enabled: boolean,
): NetworkRule | undefined {
  const list = queryClient.getQueryData<NetworkRuleList>(RULES_KEY)
  const previous = list?.rules.find((rule) => rule.id === ruleId)
  queryClient.setQueryData<NetworkRuleList>(RULES_KEY, (existing) =>
    existing
      ? {
          rules: existing.rules.map((rule) =>
            rule.id === ruleId ? { ...rule, enabled } : rule,
          ),
        }
      : existing,
  )
  return previous
}

export function rollbackRule(
  queryClient: QueryClient,
  previous: NetworkRule | undefined,
): void {
  if (!previous) return
  queryClient.setQueryData<NetworkRuleList>(RULES_KEY, (existing) => ({
    rules: upsertRule(existing?.rules ?? [], previous),
  }))
}

export function settleRule(queryClient: QueryClient, next: NetworkRule): void {
  queryClient.setQueryData<NetworkRuleList>(RULES_KEY, (existing) => ({
    rules: upsertRule(existing?.rules ?? [], next),
  }))
}

export interface NetworkScreenData {
  status?: NetworkStatus
  statusPending: boolean
  statusError?: Error
  refetchStatus(): void
  rules: NetworkRule[]
  rulesPending: boolean
  records: DerivedRecord[]
  totalRecords: number
  connection: ConnectionState
  rulesById: Map<string, NetworkRule>
  actions: {
    startCapture(): Promise<unknown>
    stopCapture(): Promise<unknown>
    clearRecords(): Promise<unknown>
    copyReference(recordId: string): Promise<{ clipboardText: string }>
    deleteRecord(recordId: string): Promise<unknown>
    restoreRecord(undoToken: string): Promise<unknown>
    toggleRule(id: string, enabled: boolean): Promise<unknown>
    renameRule(id: string, name: string): Promise<unknown>
    setScope(
      id: string,
      scope: 'all_tabs' | 'tab',
      tabId?: number,
    ): Promise<unknown>
    deleteRule(id: string): Promise<unknown>
    restoreRule(id: string): Promise<unknown>
    deleteAllRules(): Promise<{ ids: string[] }>
    restoreRules(ids: string[]): Promise<{ ok: number; failed: number }>
    stopAll(): Promise<unknown>
    testRule(id: string): Promise<unknown>
    reloadTab(tabId: number, ignoreCache: boolean): Promise<unknown>
    restoreRevision(id: string, revision: number): Promise<unknown>
  }
}

export function useNetworkScreenData(): NetworkScreenData {
  const queryClient = useQueryClient()
  const statusQuery = useNetworkStatus()
  const rulesQuery = useNetworkRules()
  const recordsQuery = useNetworkRecords({
    variables: { limit: RECORD_RENDER_LIMIT },
  })

  const storeRef = useRef<ReturnType<typeof createRecordsStore> | null>(null)
  if (!storeRef.current)
    storeRef.current = createRecordsStore(RECORD_RENDER_LIMIT)
  const store = storeRef.current
  const snapshot = useSyncExternalStore(
    store.subscribe,
    store.getSnapshot,
    store.getSnapshot,
  )

  const [connection, setConnection] = useState<ConnectionState>('connecting')

  useEffect(() => {
    if (recordsQuery.data) store.replaceAll(recordsQuery.data.records ?? [])
  }, [recordsQuery.data, store])

  useEffect(() => {
    const dispose = connectNetworkEvents({
      onEvent: (event) => {
        if (event.type === 'gap' && event.data?.reset) {
          void recordsQuery.refetch()
          void queryClient.invalidateQueries({ queryKey: STATUS_KEY })
          void queryClient.invalidateQueries({ queryKey: RULES_KEY })
          return
        }
        applyNetworkEvent(queryClient, store, event)
      },
      onState: setConnection,
      onOpen: () => {
        void recordsQuery.refetch()
        void queryClient.invalidateQueries({ queryKey: STATUS_KEY })
        void queryClient.invalidateQueries({ queryKey: RULES_KEY })
      },
    })
    return dispose
    // `recordsQuery` is stable across renders; refetch is captured once.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [queryClient, store, recordsQuery.refetch])

  const startCapture = useStartNetworkCapture()
  const stopCapture = useStopNetworkCapture()
  const deleteRecords = useDeleteNetworkRecords()
  const createHandoff = useCreateNetworkHandoff()
  const deleteRecord = useDeleteNetworkRecord()
  const restoreRecord = useRestoreNetworkRecord()
  const patchRule = usePatchNetworkRule()
  const deleteRule = useDeleteNetworkRule()
  const restoreRule = useRestoreNetworkRule()
  const deleteAllRules = useDeleteAllNetworkRules()
  const restoreRevision = useRestoreNetworkRuleRevision()
  const testRule = useTestNetworkRule()
  const stopAll = useStopAllNetworkMocks()
  const reloadTab = useReloadNetworkTab()

  const toggleRule = useCallback(
    async (id: string, enabled: boolean) => {
      const previous = toggleRuleOptimistic(queryClient, id, enabled)
      try {
        const updated = await patchRule.mutateAsync({ id, enabled })
        settleRule(queryClient, updated as unknown as NetworkRule)
        return updated
      } catch (error) {
        rollbackRule(queryClient, previous)
        throw error
      }
    },
    [patchRule, queryClient],
  )

  const rules = rulesQuery.data?.rules ?? []
  const records = snapshot.order
    .map((id) => snapshot.byId.get(id))
    .filter((record): record is DerivedRecord => Boolean(record))

  return {
    status: statusQuery.data,
    statusPending: statusQuery.isPending,
    statusError: statusQuery.error ?? undefined,
    refetchStatus: () => void statusQuery.refetch(),
    rules: sortRules(rules),
    rulesPending: rulesQuery.isPending,
    records,
    totalRecords: statusQuery.data?.capture.records ?? records.length,
    connection,
    rulesById: new Map(rules.map((rule) => [rule.id, rule])),
    actions: {
      startCapture: () => startCapture.mutateAsync(),
      stopCapture: () => stopCapture.mutateAsync(),
      clearRecords: () => deleteRecords.mutateAsync(),
      copyReference: (recordId) => createHandoff.mutateAsync({ recordId }),
      deleteRecord: (recordId) => deleteRecord.mutateAsync({ id: recordId }),
      restoreRecord: (undoToken) => restoreRecord.mutateAsync({ undoToken }),
      toggleRule,
      renameRule: (id, name) => patchRule.mutateAsync({ id, name }),
      setScope: (id, scope, tabId) =>
        patchRule.mutateAsync({ id, scope, tabId }),
      deleteRule: (id) => deleteRule.mutateAsync({ id }),
      restoreRule: (id) => restoreRule.mutateAsync({ id }),
      deleteAllRules: async () => {
        const ids = (rulesQuery.data?.rules ?? []).map((rule) => rule.id)
        await deleteAllRules.mutateAsync()
        return { ids }
      },
      restoreRules: async (ids) => {
        const settled = await Promise.allSettled(
          ids.map((id) => restoreRule.mutateAsync({ id })),
        )
        const ok = settled.filter((item) => item.status === 'fulfilled').length
        return { ok, failed: settled.length - ok }
      },
      stopAll: () => stopAll.mutateAsync(),
      testRule: (id) => testRule.mutateAsync({ id }),
      reloadTab: (tabId, ignoreCache) =>
        reloadTab.mutateAsync({ tabId, ignoreCache }),
      restoreRevision: (id, revision) =>
        restoreRevision.mutateAsync({ id, revision }),
    },
  }
}
