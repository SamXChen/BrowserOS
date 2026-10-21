import { describe, expect, it, mock } from 'bun:test'
import type {
  NetworkMatcher,
  NetworkRecord,
  NetworkRule,
} from '@browseros/claw-api'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { renderToStaticMarkup } from 'react-dom/server'
import { MemoryRouter } from 'react-router'

function matcher(overrides: Partial<NetworkMatcher> = {}): NetworkMatcher {
  return {
    method: 'GET',
    origin: 'https://shop.test',
    pathname: '/api/items',
    queryPolicy: { mode: 'lenient', ignoreKeys: [] },
    bodyFields: [],
    matcherVersion: 1,
    ...overrides,
  }
}

function rule(overrides: Partial<NetworkRule> = {}): NetworkRule {
  return {
    id: 'rule_1',
    name: '商品列表',
    scope: 'all_tabs',
    enabled: true,
    state: 'active',
    currentRevision: 1,
    createdAt: 1,
    updatedAt: 1,
    hitCount: 0,
    action: { kind: 'static', status: 200, delayMs: 0, headers: {} },
    matcher: {
      method: 'GET',
      origin: 'https://shop.test',
      pathname: '/api/items',
      queryPolicy: { mode: 'lenient', ignoreKeys: [] },
      bodyFields: [],
      matcherVersion: 1,
    },
    ...overrides,
  }
}

function record(overrides: Partial<NetworkRecord> = {}): NetworkRecord {
  return {
    recordId: 'netrec_1',
    seq: 1,
    requestId: 'req_1',
    method: 'GET',
    url: 'https://shop.test/api/items?page=1',
    startedAtMs: 1_750_000_000_000,
    state: 'done',
    bodyState: 'available',
    status: 200,
    encodedDataLength: 128,
    tabTitle: '商店',
    ...overrides,
  }
}

function processorRule(overrides: Partial<NetworkRule> = {}): NetworkRule {
  return rule({
    name: '价格归零',
    action: {
      kind: 'processor',
      script:
        'function process({ response }) {\n  return { body: response.json() }\n}',
      timeoutMs: 200,
    },
    ...overrides,
  })
}

const noop = async () => ({})

const baseData = {
  status: {
    browser: { connected: true },
    capture: {
      running: true,
      records: 3,
      gapEvents: 0,
      tabs: [
        {
          tabId: 1,
          pageId: 1,
          title: '商店',
          url: 'https://shop.test',
          attached: true,
        },
      ],
    },
    rules: { active: 1, total: 2 },
    runtime: { paused: 0, decisionP95Ms: 0, rulesGeneration: 1 },
  },
  statusPending: false,
  statusError: undefined as Error | undefined,
  refetchStatus: () => {},
  rules: [rule(), rule({ id: 'rule_2', name: '等待中', state: 'pending' })],
  rulesPending: false,
  records: [record(), record({ recordId: 'netrec_2', seq: 2 })],
  totalRecords: 3,
  connection: 'open' as const,
  rulesById: new Map(),
  actions: {
    startCapture: noop,
    stopCapture: noop,
    clearRecords: noop,
    copyReference: noop,
    deleteRecord: noop,
    restoreRecord: noop,
    toggleRule: noop,
    renameRule: noop,
    setScope: noop,
    deleteRule: noop,
    restoreRule: noop,
    deleteAllRules: noop,
    restoreRules: noop,
    stopAll: noop,
    testRule: noop,
    reloadTab: noop,
    restoreRevision: noop,
  },
}

let dataOverride = baseData

mock.module('./network.data', () => ({
  useNetworkScreenData: () => dataOverride,
  shouldLoadResponseBody: (tab: string, bodyState?: string) =>
    tab === 'response' && bodyState === 'available',
}))

const { Network } = await import('./Network')
const { pickableTabs } = await import('@/components/network/TabPicker')
const { filterRules } = await import('@/components/network/RulesPanel')
const { shouldShowCacheHint } = await import('@/components/network/RuleCard')
const { nameEditorReducer, validateRuleName } = await import(
  '@/components/network/RuleNameEditor'
)
const { mockBadgeView } = await import('@/components/network/MockBadge')
const { ProcessorPanel } = await import('@/components/network/ProcessorPanel')

function renderProcessorPanel(sampleAvailable: boolean): string {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  })
  return renderToStaticMarkup(
    <QueryClientProvider client={queryClient}>
      <ProcessorPanel
        ruleId="rule_1"
        sampleAvailable={sampleAvailable}
        script={'function process({ response }) {\n  return response.json()\n}'}
        timeoutMs={200}
      />
    </QueryClientProvider>,
  )
}

function render(path = '/'): string {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  })
  return renderToStaticMarkup(
    <QueryClientProvider client={queryClient}>
      <MemoryRouter initialEntries={[path]}>
        <Network />
      </MemoryRouter>
    </QueryClientProvider>,
  )
}

describe('Network screen', () => {
  it('renders a skeleton while the status query is pending', () => {
    dataOverride = {
      ...baseData,
      statusPending: true,
      status: undefined as never,
    }
    const html = render()
    expect(html).toContain('animate-pulse')
    expect(html).toContain('连接中…')
    dataOverride = baseData
  })

  it('renders an error with a retry button', () => {
    dataOverride = { ...baseData, statusError: new Error('boom') }
    const html = render()
    expect(html).toContain('无法读取网络数据：boom')
    expect(html).toContain('重试')
    dataOverride = baseData
  })

  it('renders rows as options with aria-selected', () => {
    const html = render()
    expect(html).toContain('role="option"')
    expect(html).toContain('aria-selected="false"')
  })

  it('gives the row icon buttons aria labels', () => {
    const html = render()
    expect(html).toContain('aria-label="复制引用"')
    expect(html).toContain('aria-label="删除此记录"')
  })

  it('shows the render limit note when the buffer is larger', () => {
    dataOverride = {
      ...baseData,
      totalRecords: 800,
      status: {
        ...baseData.status,
        capture: { ...baseData.status.capture, records: 800 },
      },
    }
    const html = render()
    expect(html).toContain('共 800 条')
    dataOverride = baseData
  })

  it('renders the rule name, type badge and state pill', () => {
    const html = render('/network?view=rules')
    expect(html).toContain('商品列表')
    expect(html).toContain('固定响应')
    expect(html).toContain('生效中')
  })

  it('has no ignore-differences menu item', () => {
    expect(render('/network?view=rules')).not.toContain('检查匹配并忽略差异')
  })

  it('counts rules by state in the header', () => {
    const html = render()
    expect(html).toContain('Mock 1 条生效中')
    expect(html).toContain('1 条等待生效')
  })

  it('renders the processor badge and timeout summary', () => {
    dataOverride = { ...baseData, rules: [processorRule()] }
    const html = render('/network?view=rules')
    expect(html).toContain('处理器')
    expect(html).toContain('处理器 · 超时 200ms')
    dataOverride = baseData
  })

  it('disables the processor sample test without a sample', () => {
    const withoutSample = renderProcessorPanel(false)
    expect(withoutSample).toContain('在样本上试运行')
    expect(withoutSample).toMatch(/<button[^>]*disabled=""[^>]*>在样本上试运行/)

    const withSample = renderProcessorPanel(true)
    expect(withSample).toContain('在样本上试运行')
    expect(withSample).not.toMatch(/disabled=""/)
  })
})

describe('mock badges', () => {
  const mark = (outcome: string, reason?: string) => ({
    ruleId: 'rule_1',
    revision: 1,
    outcome,
    reason,
  })

  it('maps every outcome to its label and title', () => {
    expect(mockBadgeView(mark('fulfilled'), 200, '商品列表')).toMatchObject({
      label: 'mock 200',
      title: '由规则「商品列表」应答，未访问上游',
    })
    expect(mockBadgeView(mark('processed'), 201, '价格归零')).toMatchObject({
      label: 'proc 201',
      title: '规则「价格归零」的处理器已改写上游响应',
    })
    expect(mockBadgeView(mark('passthrough')).label).toBe('proc 放行')
    expect(mockBadgeView(mark('processor_error', 'boom')).label).toBe(
      'proc 失败',
    )
    expect(mockBadgeView(mark('processor_timeout', 'timeout')).label).toBe(
      'proc 失败',
    )
    expect(mockBadgeView(mark('processor_skipped', 'streaming'))).toMatchObject(
      { label: 'proc 跳过', title: '未执行处理器：流式响应' },
    )
  })
})

describe('rule helpers', () => {
  it('shows the cache hint only for active rules without hits', () => {
    const now = 100_000
    expect(
      shouldShowCacheHint(rule({ updatedAt: 1_000, hitCount: 0 }), now),
    ).toBe(true)
    expect(
      shouldShowCacheHint(rule({ updatedAt: 90_000, hitCount: 0 }), now),
    ).toBe(false)
    expect(
      shouldShowCacheHint(rule({ updatedAt: 1_000, hitCount: 2 }), now),
    ).toBe(false)
    expect(
      shouldShowCacheHint(rule({ updatedAt: 1_000, state: 'pending' }), now),
    ).toBe(false)
  })

  it('excludes NEOB and extension pages from the tab picker', () => {
    const tabs = pickableTabs(
      [
        {
          tabId: 1,
          pageId: 1,
          title: 'NEOB',
          url: 'chrome://newtab/',
          attached: true,
        },
        {
          tabId: 2,
          pageId: 2,
          title: '扩展',
          url: 'chrome-extension://abc/newtab.html',
          attached: true,
        },
        {
          tabId: 3,
          pageId: 3,
          title: '商店',
          url: 'https://shop.test',
          attached: true,
        },
      ],
      3,
    )
    expect(tabs.map((tab) => tab.tabId)).toEqual([3])
  })

  it('matches rules by name and url', () => {
    const rules = [
      rule(),
      rule({
        id: 'rule_2',
        name: '订单',
        origin: 'https://shop.test',
        pathname: '/api/orders',
        matcher: matcher({ pathname: '/api/orders' }),
      }),
      rule({
        id: 'rule_3',
        name: '别的',
        state: 'disabled',
        matcher: matcher({
          origin: 'https://other.test',
          pathname: '/api/other',
        }),
      }),
    ]
    expect(filterRules(rules, '订单', 'all').map((item) => item.id)).toEqual([
      'rule_2',
    ])
    expect(
      filterRules(rules, 'api/items', 'all').map((item) => item.id),
    ).toEqual(['rule_1'])
    expect(filterRules(rules, '', 'disabled').map((item) => item.id)).toEqual([
      'rule_3',
    ])
  })
})

describe('rule name editor', () => {
  it('validates and submits on enter', () => {
    expect(validateRuleName('  商品列表-空数据  ')).toBeNull()
    expect(validateRuleName('')).toBe('名称需为 1–60 个字符')
    expect(validateRuleName('rule_x')).toBe('名称不能以 rule_ 或 netref_ 开头')

    const editing = nameEditorReducer(
      { editing: false, value: '', error: null },
      { type: 'start', value: '商品列表' },
    )
    expect(editing.editing).toBe(true)
    const submitted = nameEditorReducer(editing, { type: 'submit' })
    expect(submitted).toEqual({
      editing: false,
      value: '商品列表',
      error: null,
    })

    const invalid = nameEditorReducer(editing, { type: 'change', value: '' })
    expect(nameEditorReducer(invalid, { type: 'submit' })).toEqual({
      editing: true,
      value: '',
      error: '名称需为 1–60 个字符',
    })
  })

  it('cancels on escape', () => {
    const editing = nameEditorReducer(
      { editing: false, value: '', error: null },
      { type: 'start', value: '旧名' },
    )
    const cancelled = nameEditorReducer(editing, { type: 'cancel' })
    expect(cancelled).toEqual({ editing: false, value: '', error: null })
  })

  it('shows a conflict when the server answers 409', () => {
    const editing = nameEditorReducer(
      { editing: false, value: '', error: null },
      { type: 'start', value: '重名' },
    )
    const conflict = nameEditorReducer(editing, { type: 'serverConflict' })
    expect(conflict.error).toBe('名称已被其他规则使用')
    expect(conflict.editing).toBe(true)
  })
})
