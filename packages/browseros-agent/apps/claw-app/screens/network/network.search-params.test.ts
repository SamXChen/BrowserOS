import { describe, expect, it } from 'bun:test'
import {
  DEFAULT_SEARCH_STATE,
  type NetworkSearchState,
  parseNetworkSearch,
  toNetworkSearch,
} from './network.search-params'

describe('network search params', () => {
  it('round trips every filter', () => {
    const state: NetworkSearchState = {
      view: 'rules',
      query: '商品 列表',
      method: 'POST',
      status: '4xx',
      resourceType: 'xhr',
      tab: '商店',
      mock: true,
      body: true,
      hideAgent: false,
      rule: 'rule_123',
      ruleState: 'attention',
    }
    expect(parseNetworkSearch(toNetworkSearch(state))).toEqual(state)
  })

  it('ignores unknown status values and defaults hideAgent to hidden', () => {
    const parsed = parseNetworkSearch(
      new URLSearchParams('status=teapot&view=somewhere&state=bogus'),
    )
    expect(parsed.status).toBe('all')
    expect(parsed.view).toBe('requests')
    expect(parsed.ruleState).toBe('all')
    expect(parsed.hideAgent).toBe(true)
  })

  it('drops empty defaults from the URL', () => {
    expect(toNetworkSearch(DEFAULT_SEARCH_STATE).toString()).toBe('')
    expect(
      toNetworkSearch({ ...DEFAULT_SEARCH_STATE, hideAgent: false }).toString(),
    ).toBe('hideAgent=0')
  })
})
