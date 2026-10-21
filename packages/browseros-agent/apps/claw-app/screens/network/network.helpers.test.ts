import { describe, expect, it } from 'bun:test'
import type {
  NetworkRecord,
  NetworkRule,
  NetworkRuleRevision,
} from '@browseros/claw-api'
import {
  DEFAULT_FILTERS,
  decodeBase64Pages,
  diffRevisions,
  filterRecords,
  isCollectorRequest,
  recordDerived,
  sortRules,
  statusTone,
} from './network.helpers'

function record(overrides: Partial<NetworkRecord> = {}): NetworkRecord {
  return {
    recordId: 'netrec_1',
    seq: 1,
    requestId: 'req_1',
    method: 'GET',
    url: 'https://shop.test/api/items?page=1',
    startedAtMs: 1_000,
    state: 'done',
    bodyState: 'available',
    ...overrides,
  }
}

function rule(overrides: Partial<NetworkRule> = {}): NetworkRule {
  return {
    id: 'rule_1',
    name: 'rule',
    scope: 'all_tabs',
    enabled: true,
    state: 'active',
    currentRevision: 1,
    createdAt: 1,
    updatedAt: 1,
    ...overrides,
  }
}

function revision(
  overrides: Partial<NetworkRuleRevision> = {},
): NetworkRuleRevision {
  return {
    revision: 1,
    matcher: {
      method: 'GET',
      origin: 'https://shop.test',
      pathname: '/api/items',
      queryPolicy: { mode: 'lenient', ignoreKeys: [] },
      bodyFields: [],
      matcherVersion: 1,
    },
    action: { kind: 'static', status: 200, headers: {} },
    source: {},
    ...overrides,
  }
}

describe('statusTone', () => {
  it('maps every bucket to one palette', () => {
    expect(statusTone(200)).toContain('emerald')
    expect(statusTone(304)).toBe('text-ink-2')
    expect(statusTone(404)).toContain('amber')
    expect(statusTone(500)).toContain('red')
    expect(statusTone(undefined, true)).toContain('red')
    expect(statusTone(undefined)).toBe('text-ink-3')
  })
})

describe('decodeBase64Pages', () => {
  it('decodes each page on its own so padding never lands mid-stream', () => {
    const first = new Uint8Array([0x7b, 0x7d]) // "{}" -> "e30=" has one pad
    const second = new Uint8Array(65_536).fill(0x41)
    // Encode chunk by chunk: spreading 65k bytes into fromCharCode overflows.
    const encode = (bytes: Uint8Array): string => {
      let binary = ''
      for (const byte of bytes) binary += String.fromCharCode(byte)
      return btoa(binary)
    }
    const pages = [
      { encoding: 'base64', data: encode(first) },
      { encoding: 'base64', data: encode(second) },
    ]
    expect(pages[0].data).toContain('=')

    const decoded = decodeBase64Pages(pages)
    expect(decoded.length).toBe(first.length + second.length)
    expect(Array.from(decoded.slice(0, 2))).toEqual([0x7b, 0x7d])
    expect(decoded[2]).toBe(0x41)
    expect(decoded[decoded.length - 1]).toBe(0x41)
  })

  it('encodes utf8 pages with TextEncoder', () => {
    const decoded = decodeBase64Pages([{ encoding: 'utf8', data: '价格' }])
    expect(new TextDecoder().decode(decoded)).toBe('价格')
  })
})

describe('sortRules', () => {
  it('orders by state priority then newest update', () => {
    const sorted = sortRules([
      rule({ id: 'd', state: 'disabled', updatedAt: 9 }),
      rule({ id: 'p', state: 'pending', updatedAt: 1 }),
      rule({ id: 'a1', state: 'active', updatedAt: 1 }),
      rule({ id: 'a2', state: 'active', updatedAt: 5 }),
      rule({ id: 'u', state: 'unbound', updatedAt: 7 }),
      rule({ id: 'f', state: 'failed', updatedAt: 7 }),
    ])
    expect(sorted.map((item) => item.id)).toEqual([
      'a2',
      'a1',
      'p',
      'u',
      'f',
      'd',
    ])
  })
})

describe('diffRevisions', () => {
  it('lists changed fields only', () => {
    const previous = revision()
    expect(diffRevisions(previous, previous)).toEqual([])

    const ignore = revision({
      revision: 2,
      matcher: {
        ...previous.matcher,
        queryPolicy: { mode: 'lenient', ignoreKeys: ['nonce'] },
      },
    })
    expect(diffRevisions(previous, ignore)).toEqual(['ignoreKeys'])

    const response = revision({
      revision: 3,
      action: { kind: 'static', status: 404, headers: {} },
    })
    expect(diffRevisions(previous, response)).toEqual(['响应状态'])
  })
})

describe('filterRecords', () => {
  it('applies tokens and every facet', () => {
    const records = [
      record({ recordId: 'a', method: 'GET', status: 200 }),
      record({
        recordId: 'b',
        method: 'POST',
        status: 500,
        url: 'https://shop.test/api/orders',
      }),
      record({
        recordId: 'c',
        method: 'GET',
        status: undefined,
        state: 'failed',
      }),
      record({ recordId: 'd', method: 'GET', status: 200, tabTitle: 'NEOB' }),
    ].map((item) => ({ ...item, derived: recordDerived(item) }))

    const ids = (filters: Partial<typeof DEFAULT_FILTERS>) =>
      filterRecords(records, { ...DEFAULT_FILTERS, ...filters }).map(
        (item) => item.recordId,
      )

    expect(ids({})).toEqual(['a', 'b', 'c'])
    expect(ids({ status: '5xx' })).toEqual(['b'])
    expect(ids({ status: 'failed' })).toEqual(['c'])
    expect(ids({ method: 'GET', status: '2xx' })).toEqual(['a'])
    expect(ids({ query: 'orders' })).toEqual(['b'])
    expect(ids({ hideAgent: false })).toEqual(['a', 'b', 'c', 'd'])
  })
})

describe('collector detection', () => {
  it('hides the panel tab, its API and the health probe', () => {
    expect(isCollectorRequest(record({ tabTitle: 'NEOB' }))).toBe(true)
    expect(
      isCollectorRequest(
        record({ url: 'http://127.0.0.1:9330/api/v1/network/state' }),
      ),
    ).toBe(true)
    expect(
      isCollectorRequest(
        record({ url: 'http://127.0.0.1:9330/system/health' }),
      ),
    ).toBe(true)
    expect(isCollectorRequest(record())).toBe(false)
  })
})
