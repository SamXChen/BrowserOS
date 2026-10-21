/**
 * Network REST surface as react-query hooks.
 *
 * Query keys stay literal here so every cache write in `network.data.ts` can
 * be grepped next to the reader it feeds.
 */

import type {
  NetworkBodyPage,
  NetworkCaptureState,
  NetworkDeleted,
  NetworkHandoffCreated,
  NetworkHandoffRequest,
  NetworkProcessorTestResult,
  NetworkRecordList,
  NetworkRecordsCleared,
  NetworkRestoreRequest,
  NetworkRestoreResult,
  NetworkRuleDetail,
  NetworkRuleList,
  NetworkRulePatch,
  NetworkRuleRevisionList,
  NetworkRulesDeleted,
  NetworkRuleTestResult,
  NetworkRuleUpdated,
  NetworkStatus,
  NetworkStopAllResult,
  NetworkTabReloadRequest,
} from '@browseros/claw-api'
import { createMutation, createQuery } from 'react-query-kit'
import { apiClient } from './client'

export interface NetworkRuleIdVariables {
  id: string
}

export interface NetworkRevisionVariables {
  id: string
  revision: number
}

export const useNetworkStatus = createQuery<NetworkStatus>({
  queryKey: ['api', 'network', 'status'],
  fetcher: async () => (await apiClient()).getNetworkState(),
})

export const useNetworkRules = createQuery<NetworkRuleList>({
  queryKey: ['api', 'network', 'rules'],
  fetcher: async () => (await apiClient()).listNetworkRules(),
})

export const useNetworkRule = createQuery<
  NetworkRuleDetail,
  NetworkRuleIdVariables
>({
  queryKey: ['api', 'network', 'rule'],
  fetcher: async ({ id }) => (await apiClient()).getNetworkRule({ id }),
})

export const useNetworkRuleRevisions = createQuery<
  NetworkRuleRevisionList,
  NetworkRuleIdVariables
>({
  queryKey: ['api', 'network', 'rule-revisions'],
  fetcher: async ({ id }) =>
    (await apiClient()).listNetworkRuleRevisions({ id }),
})

export interface NetworkBodyVariables {
  recordId: string
  offset: number
  limit: number
}

export const useNetworkResponseBody = createQuery<
  NetworkBodyPage,
  NetworkBodyVariables
>({
  queryKey: ['api', 'network', 'response-body'],
  staleTime: Number.POSITIVE_INFINITY,
  fetcher: async ({ recordId, offset, limit }) =>
    (await apiClient()).getNetworkResponseBody({ recordId, offset, limit }),
})

export const useNetworkRequestBody = createQuery<
  NetworkBodyPage,
  { recordId: string; offset: number; limit: number }
>({
  queryKey: ['api', 'network', 'request-body'],
  staleTime: Number.POSITIVE_INFINITY,
  fetcher: async ({ recordId, offset, limit }) =>
    (await apiClient()).getNetworkRequestBody({ recordId, offset, limit }),
})

export const useNetworkRecords = createQuery<
  NetworkRecordList,
  { limit: number }
>({
  queryKey: ['api', 'network', 'records'],
  fetcher: async ({ limit }) =>
    (await apiClient()).listNetworkRecords({ limit }),
})

export const useStartNetworkCapture = createMutation<NetworkCaptureState, void>(
  {
    mutationFn: async () => (await apiClient()).startNetworkCapture(),
  },
)

export const useStopNetworkCapture = createMutation<NetworkCaptureState, void>({
  mutationFn: async () => (await apiClient()).stopNetworkCapture(),
})

export const useDeleteNetworkRecords = createMutation<
  NetworkRecordsCleared,
  void
>({
  mutationFn: async () => (await apiClient()).deleteNetworkRecords(),
})

export const useCreateNetworkHandoff = createMutation<
  NetworkHandoffCreated,
  NetworkHandoffRequest
>({
  mutationFn: async (body) => (await apiClient()).createNetworkHandoff(body),
})

export const useDeleteNetworkRecord = createMutation<
  NetworkDeleted,
  NetworkRuleIdVariables
>({
  mutationFn: async ({ id }) =>
    (await apiClient()).deleteNetworkRecord({ recordId: id }),
})

export const useRestoreNetworkRecord = createMutation<
  NetworkRestoreResult,
  NetworkRestoreRequest
>({
  mutationFn: async (body) => (await apiClient()).restoreNetworkRecord(body),
})

export const usePatchNetworkRule = createMutation<
  NetworkRuleUpdated,
  NetworkRuleIdVariables & NetworkRulePatch
>({
  mutationFn: async ({ id, ...body }) =>
    (await apiClient()).patchNetworkRule({ id }, body),
})

export const useDeleteNetworkRule = createMutation<
  NetworkDeleted,
  NetworkRuleIdVariables
>({
  mutationFn: async ({ id }) => (await apiClient()).deleteNetworkRule({ id }),
})

export const useRestoreNetworkRule = createMutation<
  NetworkRestoreResult,
  NetworkRuleIdVariables
>({
  mutationFn: async ({ id }) => (await apiClient()).restoreNetworkRule({ id }),
})

export const useDeleteAllNetworkRules = createMutation<
  NetworkRulesDeleted,
  void
>({
  mutationFn: async () => (await apiClient()).deleteAllNetworkRules(),
})

export const useRestoreNetworkRuleRevision = createMutation<
  NetworkRuleUpdated,
  NetworkRevisionVariables
>({
  mutationFn: async ({ id, revision }) =>
    (await apiClient()).restoreNetworkRuleRevision({ id, revision }),
})

export const useTestNetworkRule = createMutation<
  NetworkRuleTestResult,
  NetworkRuleIdVariables
>({
  mutationFn: async ({ id }) => (await apiClient()).testNetworkRule({ id }),
})

export const useTestNetworkProcessor = createMutation<
  NetworkProcessorTestResult,
  NetworkRuleIdVariables
>({
  mutationFn: async ({ id }) =>
    (await apiClient()).testNetworkProcessor({ id }),
})

export const useStopAllNetworkMocks = createMutation<
  NetworkStopAllResult,
  void
>({
  mutationFn: async () => (await apiClient()).stopAllNetworkMocks(),
})

export const useReloadNetworkTab = createMutation<
  void,
  NetworkTabReloadRequest & { tabId: number }
>({
  mutationFn: async ({ tabId, ...body }) =>
    (await apiClient()).reloadNetworkTab({ tabId }, body),
})
