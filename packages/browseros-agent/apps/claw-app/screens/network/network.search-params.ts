/**
 * URL ⇄ screen state for the Network page. The URL is the single source
 * of truth: back/forward restores the tab, the filters and the expanded rule.
 */

import { type RecordFacet, STATUS_FACETS } from './network.helpers'

export type NetworkView = 'requests' | 'rules'
export type RuleStateFilter =
  | 'all'
  | 'active'
  | 'pending'
  | 'attention'
  | 'disabled'

export const RULE_STATE_FILTERS: RuleStateFilter[] = [
  'all',
  'active',
  'pending',
  'attention',
  'disabled',
]

export interface NetworkSearchState {
  view: NetworkView
  query: string
  method: string
  status: RecordFacet
  resourceType: string
  tab: string
  mock: boolean
  body: boolean
  hideAgent: boolean
  rule: string | null
  ruleState: RuleStateFilter
}

export const DEFAULT_SEARCH_STATE: NetworkSearchState = {
  view: 'requests',
  query: '',
  method: 'all',
  status: 'all',
  resourceType: 'all',
  tab: 'all',
  mock: false,
  body: false,
  hideAgent: true,
  rule: null,
  ruleState: 'all',
}

const KEYS = {
  view: 'view',
  query: 'q',
  method: 'method',
  status: 'status',
  resourceType: 'type',
  tab: 'tab',
  mock: 'mock',
  body: 'body',
  hideAgent: 'hideAgent',
  rule: 'rule',
  ruleState: 'state',
} as const

export function parseNetworkSearch(
  params: URLSearchParams,
): NetworkSearchState {
  const status = params.get(KEYS.status) ?? ''
  const view = params.get(KEYS.view)
  const ruleState = params.get(KEYS.ruleState)
  const hideAgent = params.get(KEYS.hideAgent)
  return {
    view: view === 'rules' ? 'rules' : 'requests',
    query: params.get(KEYS.query) ?? '',
    method: params.get(KEYS.method) ?? 'all',
    status: STATUS_FACETS.includes(status as RecordFacet)
      ? (status as RecordFacet)
      : 'all',
    resourceType: params.get(KEYS.resourceType) ?? 'all',
    tab: params.get(KEYS.tab) ?? 'all',
    mock: params.get(KEYS.mock) === '1',
    body: params.get(KEYS.body) === '1',
    // Hidden by default: only an explicit `hideAgent=0` shows the panel's own
    // requests, matching the previous behaviour.
    hideAgent: hideAgent === null ? true : hideAgent !== '0',
    rule: params.get(KEYS.rule),
    ruleState: RULE_STATE_FILTERS.includes(ruleState as RuleStateFilter)
      ? (ruleState as RuleStateFilter)
      : 'all',
  }
}

export function toNetworkSearch(state: NetworkSearchState): URLSearchParams {
  const params = new URLSearchParams()
  if (state.view === 'rules') params.set(KEYS.view, 'rules')
  if (state.query.trim()) params.set(KEYS.query, state.query.trim())
  if (state.method !== 'all') params.set(KEYS.method, state.method)
  if (state.status !== 'all') params.set(KEYS.status, state.status)
  if (state.resourceType !== 'all') {
    params.set(KEYS.resourceType, state.resourceType)
  }
  if (state.tab !== 'all') params.set(KEYS.tab, state.tab)
  if (state.mock) params.set(KEYS.mock, '1')
  if (state.body) params.set(KEYS.body, '1')
  if (!state.hideAgent) params.set(KEYS.hideAgent, '0')
  if (state.rule) params.set(KEYS.rule, state.rule)
  if (state.ruleState !== 'all') params.set(KEYS.ruleState, state.ruleState)
  return params
}
