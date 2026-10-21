import { describe, expect, it, mock } from 'bun:test'
import type { NetworkRule, NetworkRuleList } from '@browseros/claw-api'
import { QueryClient } from '@tanstack/react-query'
import {
  applyNetworkEvent,
  isRulePending,
  RULES_KEY,
  rollbackRule,
  shouldLoadResponseBody,
  toggleRuleOptimistic,
} from './network.data'
import { createRecordsStore } from './network.records-store'

function rule(overrides: Partial<NetworkRule> = {}): NetworkRule {
  return {
    id: 'rule_1',
    name: '商品列表',
    scope: 'all_tabs',
    enabled: true,
    state: 'active',
    currentRevision: 1,
    createdAt: 1,
    updatedAt: 10,
    ...overrides,
  }
}

function client(rules: NetworkRule[]) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  })
  queryClient.setQueryData<NetworkRuleList>(RULES_KEY, { rules })
  return queryClient
}

describe('applyNetworkEvent', () => {
  it('upserts a rule from the stream without refetching the list', () => {
    const queryClient = client([rule(), rule({ id: 'rule_2', name: '别的' })])
    const invalidate = mock(() => Promise.resolve())
    queryClient.invalidateQueries = invalidate as never

    applyNetworkEvent(queryClient, createRecordsStore(), {
      type: 'rule',
      data: rule({ name: '改名后', currentRevision: 2, updatedAt: 20 }),
    })

    const list = queryClient.getQueryData<NetworkRuleList>(RULES_KEY)
    expect(list?.rules).toHaveLength(2)
    expect(list?.rules[0].name).toBe('改名后')
    expect(invalidate).not.toHaveBeenCalled()
  })

  it('refreshes a cached detail only when the revision moved', () => {
    const queryClient = client([rule()])
    queryClient.setQueryData(['api', 'network', 'rule', { id: 'rule_1' }], {
      rule: rule({ currentRevision: 1, updatedAt: 10 }),
    })
    const invalidate = mock(() => Promise.resolve())
    queryClient.invalidateQueries = invalidate as never

    applyNetworkEvent(queryClient, createRecordsStore(), {
      type: 'rule',
      data: rule({ currentRevision: 1, updatedAt: 10 }),
    })
    expect(invalidate).not.toHaveBeenCalled()

    applyNetworkEvent(queryClient, createRecordsStore(), {
      type: 'rule',
      data: rule({ currentRevision: 2, updatedAt: 11 }),
    })
    expect(invalidate).toHaveBeenCalled()
  })

  it('counts hits only for fulfilled or processed outcomes', () => {
    const queryClient = client([rule({ hitCount: 1 })])
    applyNetworkEvent(queryClient, createRecordsStore(), {
      type: 'hit',
      data: {
        id: 'hit_1',
        ruleId: 'rule_1',
        revision: 1,
        at: 5,
        url: 'https://shop.test/api/items',
        method: 'GET',
        outcome: 'failed',
      },
    })
    const after = queryClient.getQueryData<NetworkRuleList>(RULES_KEY)
    expect(after?.rules[0].hitCount).toBe(1)
    expect(after?.rules[0].lastHit?.outcome).toBe('failed')
  })
})

describe('response body loading', () => {
  it('waits for the Response tab and an available body', () => {
    expect(shouldLoadResponseBody('general', 'available')).toBe(false)
    expect(shouldLoadResponseBody('response', 'pending')).toBe(false)
    expect(shouldLoadResponseBody('response', 'available')).toBe(true)
  })
})

describe('toggleRuleOptimistic', () => {
  it('rolls back to the exact previous snapshot', () => {
    const original = rule()
    const other = rule({ id: 'rule_2' })
    const queryClient = client([original, other])

    const previous = toggleRuleOptimistic(queryClient, 'rule_1', false)
    expect(previous).toEqual(original)
    let list = queryClient.getQueryData<NetworkRuleList>(RULES_KEY)
    expect(list?.rules[0].enabled).toBe(false)

    rollbackRule(queryClient, previous)
    list = queryClient.getQueryData<NetworkRuleList>(RULES_KEY)
    expect(list?.rules[0]).toEqual(original)
    expect(list?.rules[1]).toBe(other as never)
  })
})

describe('per-object pending state', () => {
  it('keeps other rules interactive while one toggles', () => {
    const mutation = { isPending: true, variables: { id: 'rule_1' } }
    expect(isRulePending(mutation, 'rule_1')).toBe(true)
    expect(isRulePending(mutation, 'rule_2')).toBe(false)
    expect(
      isRulePending(
        { isPending: false, variables: { id: 'rule_1' } },
        'rule_1',
      ),
    ).toBe(false)
  })
})
