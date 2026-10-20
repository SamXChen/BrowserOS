import {
  Ban,
  ChevronDown,
  Copy,
  MoreHorizontal,
  Search,
  Trash2,
  X,
} from 'lucide-react'
import { type ReactNode, useEffect, useMemo, useRef, useState } from 'react'
import { useSearchParams } from 'react-router'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { Switch } from '@/components/ui/switch'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { cn } from '@/lib/utils'
import {
  type BodyPage,
  type HeaderPair,
  type NetworkRecord,
  type NetworkRule,
  networkApi,
  type RuleDetail,
  type RuleRevision,
} from '@/modules/api/network.api'
import { useNetworkScreen } from './network.data'

const ROW_GRID =
  'grid grid-cols-[58px_minmax(0,1fr)_46px_62px_90px_78px] items-center gap-3'

type StatusFilter = 'all' | '2xx' | '3xx' | '4xx' | '5xx' | 'failed' | 'pending'

const METHODS = ['GET', 'POST', 'PUT', 'PATCH', 'DELETE', 'OPTIONS']
const STATUSES: StatusFilter[] = [
  'all',
  '2xx',
  '3xx',
  '4xx',
  '5xx',
  'failed',
  'pending',
]

function isCollectorRequest(record: NetworkRecord): boolean {
  // The Network screen itself lives in the NEOB tab. Hiding everything from
  // that tab covers its API polls, SSE and settings reads without hiding a
  // normal page that happens to call the same local server.
  if (record.tabTitle === 'NEOB') return true
  try {
    const { pathname } = new URL(record.url)
    return (
      pathname.startsWith('/api/v1/network') || pathname === '/system/health'
    )
  } catch {
    return false
  }
}

function matchesStatus(record: NetworkRecord, filter: StatusFilter): boolean {
  if (filter === 'all') return true
  if (filter === 'failed') return record.state === 'failed'
  if (filter === 'pending') {
    return record.state !== 'failed' && record.status === undefined
  }
  if (record.status === undefined) return false
  const bucket = `${Math.floor(record.status / 100)}xx`
  return bucket === filter
}

function methodTone(method: string): string {
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

// biome-ignore lint/complexity/noExcessiveCognitiveComplexity: the screen owns capture, filters and rule controls
export function Network() {
  const screen = useNetworkScreen()
  const [params, setParams] = useSearchParams()
  const [tab, setTab] = useState<'requests' | 'rules'>('requests')
  const [query, setQuery] = useState(() => params.get('q') ?? '')
  const [method, setMethod] = useState(() => params.get('method') ?? 'all')
  const [status, setStatus] = useState<StatusFilter>(() => {
    const value = params.get('status') ?? 'all'
    return STATUSES.includes(value as StatusFilter)
      ? (value as StatusFilter)
      : 'all'
  })
  const [resourceType, setResourceType] = useState(
    () => params.get('type') ?? 'all',
  )
  const [sourceTab, setSourceTab] = useState(() => params.get('tab') ?? 'all')
  const [mockOnly, setMockOnly] = useState(() => params.get('mock') === '1')
  const [hasBody, setHasBody] = useState(() => params.get('body') === '1')
  const [hideCollector, setHideCollector] = useState(
    () => params.get('hideAgent') !== '0',
  )
  const [confirmClear, setConfirmClear] = useState(false)
  const searchRef = useRef<HTMLInputElement>(null)
  const running = screen.status?.capture.running ?? false
  const paused = running && screen.status?.browser.connected === false
  const activeRules = screen.rules.filter((rule) => rule.enabled).length
  const pendingRules = screen.rules.filter(
    (rule) => rule.enabled && !rule.lastHit,
  ).length

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== '/' || event.metaKey || event.ctrlKey || event.altKey) {
        return
      }
      const target = event.target as HTMLElement | null
      if (
        target &&
        (target.tagName === 'INPUT' ||
          target.tagName === 'TEXTAREA' ||
          target.isContentEditable)
      ) {
        return
      }
      event.preventDefault()
      searchRef.current?.focus()
    }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [])

  useEffect(() => {
    const next = new URLSearchParams()
    if (query.trim()) next.set('q', query)
    if (method !== 'all') next.set('method', method)
    if (status !== 'all') next.set('status', status)
    if (resourceType !== 'all') next.set('type', resourceType)
    if (sourceTab !== 'all') next.set('tab', sourceTab)
    if (mockOnly) next.set('mock', '1')
    if (hasBody) next.set('body', '1')
    if (!hideCollector) next.set('hideAgent', '0')
    if (next.toString() !== params.toString()) {
      setParams(next, { replace: true })
    }
  }, [
    query,
    method,
    status,
    resourceType,
    sourceTab,
    mockOnly,
    hasBody,
    hideCollector,
    params,
    setParams,
  ])

  const collectorCount = useMemo(
    () => screen.records.filter((record) => isCollectorRequest(record)).length,
    [screen.records],
  )
  const typeOptions = useMemo(
    () =>
      Array.from(
        new Set(
          screen.records
            .map((record) => record.resourceType)
            .filter((value): value is string => Boolean(value)),
        ),
      ).sort(),
    [screen.records],
  )
  const tabOptions = useMemo(
    () =>
      Array.from(
        new Set(
          screen.records
            .map((record) => record.tabTitle)
            .filter((value): value is string => Boolean(value)),
        ),
      ).sort(),
    [screen.records],
  )

  const filteredRecords = useMemo(() => {
    const tokens = query.trim().toLowerCase().split(/\s+/).filter(Boolean)
    // biome-ignore lint/complexity/noExcessiveCognitiveComplexity: one predicate covers all filter facets
    return screen.records.filter((record) => {
      if (hideCollector && isCollectorRequest(record)) return false
      if (method !== 'all' && record.method.toUpperCase() !== method) {
        return false
      }
      if (!matchesStatus(record, status)) return false
      if (
        resourceType !== 'all' &&
        (record.resourceType ?? '') !== resourceType
      ) {
        return false
      }
      if (sourceTab !== 'all' && (record.tabTitle ?? '') !== sourceTab) {
        return false
      }
      if (mockOnly && !record.mock) return false
      if (hasBody && record.bodyState !== 'available') return false
      if (tokens.length === 0) return true
      const haystack =
        `${record.method} ${record.url} ${record.tabTitle ?? ''} ${
          record.resourceType ?? ''
        }`.toLowerCase()
      return tokens.every((token) => haystack.includes(token))
    })
  }, [
    screen.records,
    query,
    method,
    status,
    resourceType,
    sourceTab,
    mockOnly,
    hasBody,
    hideCollector,
  ])

  const hasFilters =
    query.trim().length > 0 ||
    method !== 'all' ||
    status !== 'all' ||
    resourceType !== 'all' ||
    sourceTab !== 'all' ||
    mockOnly ||
    hasBody
  const resetFilters = () => {
    setQuery('')
    setMethod('all')
    setStatus('all')
    setResourceType('all')
    setSourceTab('all')
    setMockOnly(false)
    setHasBody(false)
  }

  return (
    <div className="relative mx-auto flex w-full max-w-7xl flex-col gap-5 px-6 pt-8 pb-16 lg:h-screen lg:overflow-hidden lg:px-8 lg:pt-6 lg:pb-6">
      <header className="flex flex-wrap items-start justify-between gap-4">
        <div className="flex max-w-2xl flex-col gap-1.5">
          <h1 className="font-extrabold text-3xl leading-tight tracking-tight md:text-4xl">
            Network
          </h1>
          <p className="text-[13.5px] text-ink-2 leading-relaxed">
            Capture requests from every tab in this browser profile, copy a
            reference, and let an agent install a mock response. chrome:// /
            devtools:// / extension pages are not captured.
          </p>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <span
            className={cn(
              'inline-flex items-center gap-1.5 rounded-full px-2.5 py-[4px] font-medium text-[12px]',
              paused
                ? 'bg-amber-500/15 text-amber-700 dark:text-amber-400'
                : running
                  ? 'bg-cyanotype-live text-cyanotype-live-ink'
                  : 'bg-card-tint text-ink-2',
            )}
          >
            <span
              className={cn(
                'size-1.5 rounded-full',
                paused
                  ? 'bg-amber-500'
                  : running
                    ? 'bg-cyanotype-live-ink'
                    : 'bg-ink-3',
              )}
            />
            {running
              ? paused
                ? '已暂停 · 浏览器未连接'
                : `采集中 · ${screen.status?.capture.records ?? 0} 条`
              : '已停止'}
          </span>
          <Button
            size="sm"
            variant={running ? 'outline' : 'default'}
            disabled={screen.busy}
            onClick={() => {
              void (running ? screen.stopCapture() : screen.startCapture())
            }}
          >
            {running ? '停止采集' : '开始采集'}
          </Button>
          <Button
            size="sm"
            variant="outline"
            disabled={screen.busy || activeRules === 0}
            onClick={() => {
              void screen.stopAll()
            }}
          >
            <Ban className="size-3.5" />
            停止全部 Mock
          </Button>
          {screen.records.length > 0 ? (
            <Button
              size="sm"
              variant="ghost"
              className="text-ink-3"
              disabled={screen.busy}
              onClick={() => setConfirmClear(true)}
            >
              清空
            </Button>
          ) : null}
        </div>
      </header>

      {screen.error ? (
        <div className="rounded-9 border border-ledger-border bg-card px-6 py-4 text-[13px] text-ink-2">
          无法读取网络数据：{screen.error}
        </div>
      ) : null}

      <Tabs
        className="flex min-h-0 flex-1 flex-col gap-4"
        onValueChange={(value) => setTab(value as 'requests' | 'rules')}
        value={tab}
      >
        <div className="flex shrink-0 flex-wrap items-center justify-between gap-3">
          <TabsList
            aria-label="Network views"
            className="h-9 rounded-[9px] bg-cyanotype-well p-1"
          >
            <TabsTrigger
              className="h-7 flex-none rounded-md border-0 px-3 font-normal text-[12px] text-cyanotype-muted data-active:bg-white data-active:font-semibold data-active:text-cyanotype-blue"
              value="requests"
            >
              Requests · {filteredRecords.length}
            </TabsTrigger>
            <TabsTrigger
              className="h-7 flex-none rounded-md border-0 px-3 font-normal text-[12px] text-cyanotype-muted data-active:bg-white data-active:font-semibold data-active:text-cyanotype-blue"
              value="rules"
            >
              Rules · {screen.rules.length}
            </TabsTrigger>
          </TabsList>
          <div className="flex items-center gap-2 text-[12px] text-ink-3">
            <span className="rounded-full bg-card-tint px-2.5 py-1 text-[11.5px] text-ink-2">
              Mock {activeRules} 条生效中
              {pendingRules > 0 ? ` · ${pendingRules} 条等待命中` : ''}
            </span>
            {running ? (
              <span
                className="text-[10.5px] text-ink-3"
                title="新标签页立即导航时，首个文档请求可能来不及采集；后续请求会正常出现"
              >
                新标签页首请求可能缺失
              </span>
            ) : null}
            {screen.status?.runtime &&
            (screen.status.runtime.paused > 0 ||
              screen.status.runtime.decisionP95Ms > 100) ? (
              <span
                className="font-mono text-[10.5px] text-ink-3"
                title={`规则快照 v${screen.status.runtime.rulesGeneration}`}
              >
                queue {screen.status.runtime.paused} · p95{' '}
                {screen.status.runtime.decisionP95Ms}ms
              </span>
            ) : null}
            {screen.status?.capture.gapEvents ? (
              <span className="text-amber-600">
                gap {screen.status.capture.gapEvents} 条事件丢失
              </span>
            ) : null}
          </div>
        </div>

        <TabsContent className="flex min-h-0 flex-1 flex-col" value="requests">
          <div className="mb-3 flex shrink-0 flex-col gap-2 rounded-9 border border-ledger-border bg-card px-3 py-2.5">
            <div className="flex flex-wrap items-center gap-2">
              <div className="relative min-w-[220px] max-w-md flex-1">
                <Search
                  className="absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2 text-ink-3"
                  aria-hidden
                />
                <input
                  ref={searchRef}
                  type="search"
                  value={query}
                  onChange={(event) => setQuery(event.target.value)}
                  placeholder="Filter URL, tab or type…  ( / )"
                  aria-label="Filter requests"
                  className="h-9 w-full rounded-md border border-ledger-border bg-background pr-8 pl-8 text-[12.5px] text-ink placeholder:text-ink-3 focus:border-cyanotype-blue/40 focus:outline-none focus:ring-2 focus:ring-cyanotype-blue/15"
                />
                {query ? (
                  <button
                    type="button"
                    aria-label="Clear filter"
                    className="absolute top-1/2 right-1.5 inline-flex size-6 -translate-y-1/2 items-center justify-center rounded text-ink-3 hover:bg-card-tint hover:text-ink"
                    onClick={() => setQuery('')}
                  >
                    <X className="size-3.5" />
                  </button>
                ) : null}
              </div>

              <FilterSelect
                label="Filter by method"
                value={method}
                onChange={setMethod}
              >
                <option value="all">All methods</option>
                {METHODS.map((item) => (
                  <option key={item} value={item}>
                    {item}
                  </option>
                ))}
              </FilterSelect>

              <FilterSelect
                label="Filter by status"
                value={status}
                onChange={(value) => setStatus(value as StatusFilter)}
              >
                <option value="all">Any status</option>
                <option value="2xx">2xx success</option>
                <option value="3xx">3xx redirect</option>
                <option value="4xx">4xx client error</option>
                <option value="5xx">5xx server error</option>
                <option value="failed">Failed</option>
                <option value="pending">Pending</option>
              </FilterSelect>

              <FilterSelect
                label="Filter by resource type"
                value={resourceType}
                onChange={setResourceType}
              >
                <option value="all">All types</option>
                {typeOptions.map((item) => (
                  <option key={item} value={item}>
                    {item}
                  </option>
                ))}
              </FilterSelect>

              <FilterSelect
                label="Filter by source tab"
                value={sourceTab}
                onChange={setSourceTab}
              >
                <option value="all">All tabs</option>
                {tabOptions.map((item) => (
                  <option key={item} value={item}>
                    {item}
                  </option>
                ))}
              </FilterSelect>

              <FilterToggle
                active={mockOnly}
                onClick={() => setMockOnly((value) => !value)}
                title="Only show requests answered by a mock rule"
              >
                Mock only
              </FilterToggle>

              <FilterToggle
                active={hasBody}
                onClick={() => setHasBody((value) => !value)}
                title="Only show requests whose response body was captured"
              >
                Has body
              </FilterToggle>

              <FilterToggle
                active={hideCollector}
                onClick={() => setHideCollector((value) => !value)}
                title="Hide requests made by this Network page to its own API"
              >
                {hideCollector ? 'Agent API hidden' : 'Agent API shown'}
                {collectorCount > 0 ? (
                  <span className="rounded-full bg-cyanotype-blue/10 px-1.5 text-[10px] text-cyanotype-blue">
                    {collectorCount}
                  </span>
                ) : null}
              </FilterToggle>

              {hasFilters ? (
                <Button
                  size="xs"
                  variant="ghost"
                  onClick={resetFilters}
                  className="text-ink-3"
                >
                  <X className="size-3" />
                  Reset
                </Button>
              ) : null}

              <span
                className="ml-auto whitespace-nowrap font-mono text-[11.5px] text-ink-3"
                title={
                  (screen.status?.capture.records ?? 0) > screen.records.length
                    ? `仅渲染最新 ${screen.records.length} 条，共 ${screen.status?.capture.records} 条`
                    : undefined
                }
              >
                {filteredRecords.length} / {screen.records.length}
                {(screen.status?.capture.records ?? 0) > screen.records.length
                  ? ` · ${screen.records.length} latest`
                  : ''}
              </span>
            </div>
          </div>

          <div className="grid min-h-0 flex-1 grid-cols-1 gap-4 lg:grid-cols-[minmax(0,1fr)_400px]">
            <RequestList
              busy={screen.busy}
              capturing={running}
              emptyKind={screen.records.length === 0 ? 'idle' : 'filtered'}
              records={filteredRecords}
              selectedId={screen.selectedId}
              onCopy={(record) => {
                void screen.copyReference(record)
              }}
              onDelete={(record) => {
                void screen.deleteRecord(record)
              }}
              onDisable={(ruleId) => {
                const rule = screen.rules.find((item) => item.id === ruleId)
                if (rule) void screen.toggleRule(rule, false)
              }}
              onReset={resetFilters}
              onSelect={screen.select}
            />
            <RequestDetail
              body={screen.detailBody}
              record={screen.selected}
              rules={screen.rules}
              onCopy={(record) => {
                void screen.copyReference(record)
              }}
              onDelete={(record) => {
                void screen.deleteRecord(record)
              }}
              onToggleRule={(rule) => {
                void screen.toggleRule(rule, !rule.enabled)
              }}
            />
          </div>
        </TabsContent>

        <TabsContent className="min-h-0 flex-1 overflow-y-auto" value="rules">
          {screen.rules.length === 0 ? (
            <div className="rounded-9 border border-ledger-border bg-card px-6 py-12 text-center text-[13px] text-ink-2">
              还没有规则。在请求上点「复制引用」，交给 AI 描述你想要的响应。
            </div>
          ) : (
            <div className="grid grid-cols-1 gap-3">
              {screen.rules.map((rule) => (
                <RuleCard
                  key={rule.id}
                  rule={rule}
                  busy={screen.busy}
                  onDelete={() => {
                    void screen.deleteRule(rule)
                  }}
                  currentTabId={
                    (screen.status?.capture.tabs ?? []).find(
                      (tab) => tab.title !== 'NEOB',
                    )?.tabId
                  }
                  onFixMatch={() => {
                    void screen.fixNearMiss(rule)
                  }}
                  onRefresh={() => {
                    void screen.refresh()
                  }}
                  onToggle={(enabled) => {
                    void screen.toggleRule(rule, enabled)
                  }}
                />
              ))}
            </div>
          )}
        </TabsContent>
      </Tabs>

      {confirmClear ? (
        <div className="fixed inset-0 z-50 flex items-center justify-center px-4">
          <button
            type="button"
            aria-label="关闭"
            className="absolute inset-0 bg-black/30"
            onClick={() => setConfirmClear(false)}
          />
          <div
            aria-modal="true"
            className="relative w-full max-w-sm rounded-9 border border-ledger-border bg-card p-5 shadow-xl"
            role="dialog"
          >
            <h2 className="font-semibold text-[15px] text-ink">
              清空采集列表？
            </h2>
            <p className="mt-1.5 text-[12.5px] text-ink-2 leading-relaxed">
              将删除 {screen.records.length} 条记录，采集会继续运行。
            </p>
            <div className="mt-4 flex justify-end gap-2">
              <Button
                size="sm"
                variant="outline"
                onClick={() => setConfirmClear(false)}
              >
                取消
              </Button>
              <Button
                size="sm"
                variant="destructive"
                onClick={() => {
                  setConfirmClear(false)
                  void screen.clearRecords()
                }}
              >
                清空
              </Button>
            </div>
          </div>
        </div>
      ) : null}
    </div>
  )
}

function FilterSelect({
  label,
  value,
  onChange,
  children,
}: {
  label: string
  value: string
  onChange: (value: string) => void
  children: ReactNode
}) {
  return (
    <div className="relative">
      <select
        value={value}
        onChange={(event) => onChange(event.target.value)}
        aria-label={label}
        className="h-9 appearance-none rounded-md border border-ledger-border bg-background pr-8 pl-2.5 text-[12px] text-ink focus:border-cyanotype-blue/40 focus:outline-none"
      >
        {children}
      </select>
      <ChevronDown
        aria-hidden
        className="pointer-events-none absolute top-1/2 right-2 size-3.5 -translate-y-1/2 text-ink-3"
      />
    </div>
  )
}

function FilterToggle({
  active,
  children,
  onClick,
  title,
}: {
  active: boolean
  children: ReactNode
  onClick: () => void
  title: string
}) {
  return (
    <button
      type="button"
      aria-pressed={active}
      title={title}
      onClick={onClick}
      className={cn(
        'inline-flex h-9 items-center gap-1.5 rounded-md border px-2.5 text-[12px] transition-colors',
        active
          ? 'border-cyanotype-blue/30 bg-cyanotype-blue/10 text-cyanotype-blue'
          : 'border-ledger-border bg-background text-ink-2 hover:bg-card-tint',
      )}
    >
      {children}
    </button>
  )
}

interface RequestListProps {
  records: NetworkRecord[]
  selectedId: string | null
  busy: boolean
  capturing: boolean
  emptyKind: 'idle' | 'filtered'
  onSelect: (recordId: string) => void
  onCopy: (record: NetworkRecord) => void
  onDelete: (record: NetworkRecord) => void
  onDisable: (ruleId: string) => void
  onReset: () => void
}

function RequestList({
  records,
  selectedId,
  busy,
  capturing,
  emptyKind,
  onSelect,
  onCopy,
  onDelete,
  onDisable,
  onReset,
}: RequestListProps) {
  const scrollRef = useRef<HTMLDivElement>(null)
  const previousFirst = useRef<string | undefined>(undefined)
  const [newCount, setNewCount] = useState(0)
  const firstId = records[0]?.recordId

  useEffect(() => {
    if (!firstId || firstId === previousFirst.current) return
    previousFirst.current = firstId
    const element = scrollRef.current
    if (element && element.scrollTop > 24) {
      setNewCount((count) => count + 1)
    } else {
      element?.scrollTo({ top: 0 })
      setNewCount(0)
    }
  }, [firstId])

  const moveSelection = (delta: number) => {
    if (records.length === 0) return
    const index = records.findIndex((record) => record.recordId === selectedId)
    const next =
      index === -1
        ? 0
        : Math.min(records.length - 1, Math.max(0, index + delta))
    const record = records[next]
    if (!record) return
    onSelect(record.recordId)
    requestAnimationFrame(() => {
      document
        .querySelector(`[data-record-id="${record.recordId}"]`)
        ?.scrollIntoView({ block: 'nearest' })
    })
  }

  return (
    <section className="relative flex min-h-[420px] min-w-0 flex-col overflow-hidden rounded-9 border border-ledger-border bg-card lg:h-full lg:min-h-0">
      <div
        className={cn(
          ROW_GRID,
          'sticky top-0 z-10 shrink-0 border-ledger-border border-b bg-ledger-head px-3 py-2 font-semibold text-[10.5px] text-ledger-head-ink uppercase tracking-[0.05em]',
        )}
      >
        <span>Time</span>
        <span>Request</span>
        <span>Status</span>
        <span className="text-right">Size</span>
        <span>Source</span>
        <span />
      </div>
      {newCount > 0 ? (
        <button
          type="button"
          className="absolute top-10 left-1/2 z-20 -translate-x-1/2 rounded-full bg-cyanotype-blue px-3 py-1 font-medium text-[11px] text-white shadow-md"
          onClick={() => {
            scrollRef.current?.scrollTo({ top: 0, behavior: 'smooth' })
            setNewCount(0)
          }}
        >
          {newCount} 条新请求 ↓
        </button>
      ) : null}

      <div
        ref={scrollRef}
        aria-label="Captured requests"
        className="min-h-0 flex-1 overflow-y-auto outline-none"
        role="listbox"
        onKeyDown={(event) => {
          if (event.key === 'ArrowDown') {
            event.preventDefault()
            moveSelection(1)
          } else if (event.key === 'ArrowUp') {
            event.preventDefault()
            moveSelection(-1)
          } else if (event.key === 'Enter' && selectedId) {
            event.preventDefault()
            const record = records.find((item) => item.recordId === selectedId)
            if (record) onCopy(record)
          }
        }}
        tabIndex={0}
      >
        {records.length === 0 ? (
          <div className="flex flex-col items-start gap-2 px-4 py-12 text-[13px] text-ink-2">
            {emptyKind === 'idle' ? (
              <p>
                {capturing
                  ? '采集中，正常操作页面即可看到请求。'
                  : '点「开始采集」，然后正常操作页面即可看到请求。'}
              </p>
            ) : (
              <>
                <p>没有请求匹配当前筛选条件。</p>
                <Button size="xs" variant="outline" onClick={onReset}>
                  清除筛选
                </Button>
              </>
            )}
          </div>
        ) : (
          records.map((record) => (
            <RequestRow
              key={record.recordId}
              busy={busy}
              record={record}
              selected={record.recordId === selectedId}
              onCopy={() => onCopy(record)}
              onDelete={() => onDelete(record)}
              onDisable={() => {
                if (record.mock) onDisable(record.mock.ruleId)
              }}
              onSelect={() => onSelect(record.recordId)}
            />
          ))
        )}
      </div>
    </section>
  )
}

interface RequestRowProps {
  record: NetworkRecord
  selected: boolean
  busy: boolean
  onSelect: () => void
  onCopy: () => void
  onDelete: () => void
  onDisable: () => void
}

function RequestRow({
  record,
  selected,
  busy,
  onSelect,
  onCopy,
  onDelete,
  onDisable,
}: RequestRowProps) {
  const parts = splitUrl(record.url)
  const source = record.tabTitle ?? '—'
  return (
    // biome-ignore lint/a11y/useSemanticElements: the row contains its own action buttons, so it cannot be a <button>
    <div
      role="button"
      tabIndex={0}
      data-record-id={record.recordId}
      title={`${record.method} ${record.url}`}
      className={cn(
        ROW_GRID,
        'group cursor-pointer border-ledger-divider border-b px-3 py-2.5 text-left transition-colors last:border-b-0 hover:bg-card-tint focus-visible:bg-card-tint focus-visible:outline-none',
        selected && 'bg-card-tint',
      )}
      onClick={onSelect}
      onKeyDown={(event) => {
        if (event.target !== event.currentTarget) return
        if (event.key === 'Enter' || event.key === ' ') {
          event.preventDefault()
          onSelect()
        }
      }}
    >
      <div
        className="flex flex-col gap-0.5 font-mono text-[11px] text-ink-3 tabular-nums"
        title={new Date(record.startedAtMs).toLocaleString()}
      >
        <span>{formatRelative(record.startedAtMs)}</span>
        <span className="text-[10px] text-ink-4">
          {record.finishedAtMs
            ? formatDuration(record.finishedAtMs - record.startedAtMs)
            : '…'}
        </span>
      </div>

      <div className="min-w-0">
        <div className="flex min-w-0 items-center gap-2">
          <span
            className={cn(
              'shrink-0 rounded px-1.5 py-0.5 font-mono font-semibold text-[10.5px]',
              methodTone(record.method),
            )}
          >
            {record.method}
          </span>
          <span
            className="truncate font-mono text-[12.5px] text-ink"
            title={record.url}
          >
            {parts.path || '/'}
          </span>
          {record.mock ? (
            <span
              className="shrink-0 rounded-full bg-cyanotype-blue/10 px-1.5 py-0.5 font-medium text-[10px] text-cyanotype-blue"
              title="已由规则应答，未访问上游"
            >
              mock {record.status ?? 200}
            </span>
          ) : null}
        </div>
        <div className="mt-0.5 flex min-w-0 items-center gap-2 text-[11px] text-ink-3">
          <span className="truncate" title={record.url}>
            {parts.host}
          </span>
          {record.resourceType ? (
            <span className="shrink-0 rounded bg-card-tint px-1 py-0.5 text-[10px] uppercase">
              {record.resourceType}
            </span>
          ) : null}
        </div>
      </div>

      <StatusCell record={record} />

      <span className="whitespace-nowrap text-right text-[11.5px] text-ink-2 tabular-nums">
        {record.encodedDataLength === undefined
          ? '—'
          : formatBytes(record.encodedDataLength)}
      </span>

      <span className="truncate text-[11.5px] text-ink-3" title={source}>
        {source}
      </span>

      <div className="flex items-center justify-end gap-0.5 opacity-0 transition-opacity focus-within:opacity-100 group-hover:opacity-100">
        <Button
          size="icon-xs"
          variant="ghost"
          title="复制引用"
          disabled={busy}
          onClick={(event) => {
            event.stopPropagation()
            onCopy()
          }}
        >
          <Copy className="size-3.5" />
        </Button>
        {record.mock ? (
          <Button
            size="icon-xs"
            variant="ghost"
            title="停用此规则"
            disabled={busy}
            onClick={(event) => {
              event.stopPropagation()
              onDisable()
            }}
          >
            <Ban className="size-3.5" />
          </Button>
        ) : null}
        <Button
          size="icon-xs"
          variant="ghost"
          title="删除此记录"
          disabled={busy}
          onClick={(event) => {
            event.stopPropagation()
            onDelete()
          }}
        >
          <Trash2 className="size-3.5" />
        </Button>
      </div>
    </div>
  )
}

function StatusCell({ record }: { record: NetworkRecord }) {
  if (record.state === 'failed') {
    return (
      <span className="text-[12px] text-red-500" title={record.failure}>
        失败
      </span>
    )
  }
  const status = record.status
  return (
    <span
      className={cn(
        'font-mono text-[12px] tabular-nums',
        status === undefined
          ? 'text-ink-3'
          : status >= 400
            ? 'text-red-500'
            : status >= 300
              ? 'text-amber-600'
              : 'text-ink',
      )}
    >
      {record.mock ? 'mock ' : ''}
      {status ?? '—'}
    </span>
  )
}

interface RequestDetailProps {
  record: NetworkRecord | null
  body: BodyPage | null
  rules: NetworkRule[]
  onCopy: (record: NetworkRecord) => void
  onDelete: (record: NetworkRecord) => void
  onToggleRule: (rule: NetworkRule) => void
}

type DetailTab = 'headers' | 'payload' | 'response'

function RequestDetail({
  record,
  body,
  rules,
  onCopy,
  onDelete,
  onToggleRule,
}: RequestDetailProps) {
  const [tab, setTab] = useState<DetailTab>('headers')

  // Reset the pane only when a different record is selected; SSE updates to
  // the same record must not yank the user back to Headers.
  // biome-ignore lint/correctness/useExhaustiveDependencies: keyed by recordId on purpose
  useEffect(() => {
    setTab('headers')
  }, [record?.recordId])

  if (!record) {
    return (
      <div className="flex min-h-[420px] items-center justify-center rounded-9 border border-ledger-border bg-card px-6 text-center text-[13px] text-ink-2 lg:h-full lg:min-h-0">
        选中一条请求查看 headers、payload 与响应。
      </div>
    )
  }

  const rule = record.mock
    ? rules.find((item) => item.id === record.mock?.ruleId)
    : undefined

  return (
    <section className="flex min-h-[420px] flex-col overflow-hidden rounded-9 border border-ledger-border bg-card lg:h-full lg:min-h-0">
      <header className="shrink-0 border-ledger-divider border-b px-4 pt-4 pb-3">
        <div className="flex items-start justify-between gap-2">
          <div className="min-w-0">
            <div className="flex items-start gap-2">
              <span
                className={cn(
                  'mt-0.5 shrink-0 rounded px-1.5 py-0.5 font-mono font-semibold text-[10.5px]',
                  methodTone(record.method),
                )}
              >
                {record.method}
              </span>
              <div
                className="min-w-0 break-all font-mono text-[12.5px] text-ink leading-5"
                title={record.url}
              >
                {record.url}
              </div>
            </div>
            <div className="mt-1.5 flex flex-wrap items-center gap-x-3 gap-y-1 text-[11px] text-ink-3">
              <span
                className={statusTone(record.status, record.state === 'failed')}
              >
                {record.state === 'failed'
                  ? 'failed'
                  : `${record.status ?? '—'}${record.statusText ? ` ${record.statusText}` : ''}`}
              </span>
              <span>{record.resourceType ?? 'Other'}</span>
              <span
                className="max-w-[160px] truncate"
                title={record.tabTitle ?? ''}
              >
                {record.tabTitle ?? '—'}
              </span>
              <span>{record.bodyState}</span>
            </div>
          </div>
          <Button
            size="icon-sm"
            variant="ghost"
            title="删除此记录"
            className="shrink-0"
            onClick={() => onDelete(record)}
          >
            <Trash2 className="size-3.5" />
          </Button>
        </div>

        <div className="mt-2.5 flex flex-wrap items-center gap-2">
          <Button size="xs" variant="outline" onClick={() => onCopy(record)}>
            <Copy className="size-3" />
            复制引用
          </Button>
          {rule ? (
            <div className="ml-auto flex min-w-0 items-center gap-2">
              <span className="max-w-[150px] truncate text-[11px] text-ink-3">
                {rule.method} {rule.pathname}
              </span>
              <Switch
                checked={rule.enabled}
                aria-label="启用或停用此规则"
                onCheckedChange={() => onToggleRule(rule)}
              />
            </div>
          ) : null}
        </div>
      </header>

      <div
        aria-label="Request details"
        className="flex shrink-0 gap-1 border-ledger-divider border-b px-3"
        role="tablist"
      >
        {(
          [
            ['headers', 'Headers'],
            ['payload', 'Payload'],
            ['response', 'Response'],
          ] as const
        ).map(([value, label]) => (
          <button
            key={value}
            type="button"
            role="tab"
            aria-selected={tab === value}
            onClick={() => setTab(value)}
            className={cn(
              'relative h-9 px-2.5 text-[12px] transition-colors',
              tab === value
                ? 'font-semibold text-ink after:absolute after:inset-x-0 after:bottom-0 after:h-0.5 after:bg-cyanotype-blue'
                : 'text-ink-3 hover:text-ink',
            )}
          >
            {label}
          </button>
        ))}
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto p-4">
        {tab === 'headers' ? <HeadersPane record={record} /> : null}
        {tab === 'payload' ? <PayloadPane record={record} /> : null}
        {tab === 'response' ? (
          <ResponsePane body={body} record={record} />
        ) : null}
      </div>
    </section>
  )
}

function HeadersPane({ record }: { record: NetworkRecord }) {
  return (
    <div className="space-y-5">
      <DetailSection title="General">
        <dl className="grid grid-cols-[88px_minmax(0,1fr)] gap-x-3 gap-y-1.5 text-[12px] leading-5">
          <Field label="Request URL" mono>
            {record.url}
          </Field>
          <Field label="Method" mono>
            {record.method}
          </Field>
          <Field label="Status">
            {record.state === 'failed'
              ? 'failed'
              : `${record.status ?? '—'} ${record.statusText ?? ''}`}
          </Field>
          <Field label="Type">
            {record.mimeType ?? record.resourceType ?? '—'}
          </Field>
          <Field label="Source">{record.tabTitle ?? '—'}</Field>
        </dl>
      </DetailSection>

      <DetailSection title="Timing">
        <dl className="grid grid-cols-[88px_minmax(0,1fr)] gap-x-3 gap-y-1.5 text-[12px] leading-5">
          <Field label="Started">
            {new Date(record.startedAtMs).toLocaleString()}
          </Field>
          <Field label="Duration">
            {record.finishedAtMs
              ? formatDuration(record.finishedAtMs - record.startedAtMs)
              : 'in flight'}
          </Field>
          <Field label="State">
            {record.state}
            {record.failure ? ` · ${record.failure}` : ''}
          </Field>
        </dl>
      </DetailSection>

      <DetailSection
        title={`Request headers (${record.requestHeaders?.length ?? 0})`}
      >
        <HeaderList headers={record.requestHeaders} />
      </DetailSection>

      <DetailSection
        title={`Response headers (${record.responseHeaders?.length ?? 0})`}
      >
        <HeaderList headers={record.responseHeaders} />
      </DetailSection>
    </div>
  )
}

function PayloadPane({ record }: { record: NetworkRecord }) {
  const [loaded, setLoaded] = useState<BodyPage | null>(null)
  const [loading, setLoading] = useState(false)

  // biome-ignore lint/correctness/useExhaustiveDependencies: reset only when the record changes
  useEffect(() => {
    setLoaded(null)
  }, [record.recordId])

  const text = loaded?.data ?? record.postData
  if (!text) {
    return (
      <div className="space-y-2">
        <EmptyPane text="This request has no captured payload." />
        <Button
          size="xs"
          variant="outline"
          disabled={loading}
          onClick={() => {
            setLoading(true)
            void networkApi
              .requestBody(record.recordId)
              .then(setLoaded)
              .finally(() => setLoading(false))
          }}
        >
          {loading ? 'Loading…' : 'Load request body'}
        </Button>
      </div>
    )
  }
  return (
    <pre className="whitespace-pre-wrap break-all rounded-md border border-ledger-divider bg-card-tint/40 p-3 font-mono text-[11.5px] text-ink-2 leading-5">
      {prettify(text)}
    </pre>
  )
}

function ResponsePane({
  body,
  record,
}: {
  body: BodyPage | null
  record: NetworkRecord
}) {
  const [pages, setPages] = useState<BodyPage[]>([])
  const [loading, setLoading] = useState(false)

  // biome-ignore lint/correctness/useExhaustiveDependencies: a new body for the same record must not clear appended pages
  useEffect(() => {
    setPages(body ? [body] : [])
  }, [record.recordId, body])

  const last = pages[pages.length - 1]
  const data = pages.map((page) => page.data).join('')
  const loadedBytes = last
    ? last.offset +
      (last.encoding === 'base64'
        ? Math.floor(last.data.length * 0.75)
        : new TextEncoder().encode(last.data).length)
    : 0
  const total = last?.total ?? 0
  const hasMore = Boolean(last) && loadedBytes < total

  const download = () => {
    if (!last) return
    const bytes =
      last.encoding === 'base64'
        ? Uint8Array.from(atob(data), (char) => char.charCodeAt(0))
        : new TextEncoder().encode(data)
    const url = URL.createObjectURL(new Blob([bytes]))
    const anchor = document.createElement('a')
    anchor.href = url
    anchor.download = `response-${record.recordId}.bin`
    anchor.click()
    URL.revokeObjectURL(url)
  }

  return (
    <div className="space-y-3">
      <div className="flex flex-wrap items-center gap-x-4 gap-y-1 text-[11px] text-ink-3">
        <span>{record.mimeType ?? 'unknown type'}</span>
        <span>{record.bodyState}</span>
        {last ? (
          <span>
            {formatBytes(loadedBytes)} / {formatBytes(total)}
          </span>
        ) : null}
      </div>
      {last ? (
        <>
          <pre className="whitespace-pre-wrap break-all rounded-md border border-ledger-divider bg-card-tint/40 p-3 font-mono text-[11.5px] text-ink-2 leading-5">
            {last.encoding === 'base64'
              ? `二进制响应（${total} 字节，base64 已省略）`
              : prettify(data)}
          </pre>
          <div className="flex flex-wrap gap-2">
            {hasMore ? (
              <Button
                size="xs"
                variant="outline"
                disabled={loading}
                onClick={() => {
                  setLoading(true)
                  void networkApi
                    .responseBody(record.recordId, 65536, loadedBytes)
                    .then((page) => setPages((current) => [...current, page]))
                    .finally(() => setLoading(false))
                }}
              >
                {loading ? 'Loading…' : 'Load more'}
              </Button>
            ) : null}
            <Button size="xs" variant="ghost" onClick={download}>
              Download
            </Button>
          </div>
        </>
      ) : (
        <EmptyPane text={record.bodyReason ?? '响应正文不可用'} />
      )}
    </div>
  )
}

function EmptyPane({ text }: { text: string }) {
  return <p className="text-[12px] text-ink-3">{text}</p>
}

function Field({
  label,
  children,
  mono = false,
}: {
  label: string
  children: ReactNode
  mono?: boolean
}) {
  return (
    <>
      <dt className="text-ink-3">{label}</dt>
      <dd
        className={cn(
          'min-w-0 break-all text-ink-2',
          mono && 'font-mono text-[11.5px]',
        )}
      >
        {children}
      </dd>
    </>
  )
}

function DetailSection({
  title,
  children,
}: {
  title: string
  children: ReactNode
}) {
  return (
    <section className="min-w-0">
      <h3 className="mb-1.5 font-semibold text-[10.5px] text-ink-3 uppercase tracking-[0.05em]">
        {title}
      </h3>
      {children}
    </section>
  )
}

function HeaderList({ headers }: { headers?: HeaderPair[] }) {
  if (!headers || headers.length === 0) {
    return <span className="text-[12px] text-ink-3">—</span>
  }
  return (
    <div className="overflow-hidden rounded-md border border-ledger-divider">
      {headers.map((header, index) => (
        <div
          className={cn(
            'grid grid-cols-[minmax(96px,132px)_minmax(0,1fr)] gap-3 border-ledger-divider border-b px-2.5 py-1.5 font-mono text-[11.5px] leading-5 last:border-b-0',
            index % 2 === 1 && 'bg-card-tint/40',
          )}
          // biome-ignore lint/suspicious/noArrayIndexKey: duplicate header names are meaningful and order is their identity
          key={`${header.name}-${index}`}
        >
          <span className="break-all text-ink-3">{header.name}</span>
          <span className="break-all text-ink-2">{header.value}</span>
        </div>
      ))}
    </div>
  )
}

function statusTone(status?: number, failed = false): string {
  if (failed) return 'text-red-500'
  if (status === undefined) return 'text-ink-3'
  if (status >= 500) return 'text-red-600 dark:text-red-400'
  if (status >= 300) return 'text-amber-600 dark:text-amber-400'
  return 'text-emerald-600 dark:text-emerald-400'
}

interface RuleCardProps {
  rule: NetworkRule
  busy: boolean
  currentTabId?: number
  onToggle: (enabled: boolean) => void
  onDelete: () => void
  onFixMatch: () => void
  onRefresh: () => void
}

function RuleCard({
  rule,
  busy,
  currentTabId,
  onToggle,
  onDelete,
  onFixMatch,
  onRefresh,
}: RuleCardProps) {
  const [expanded, setExpanded] = useState(false)
  const replacedBy = rule.stateReason?.startsWith('replaced_by:')
    ? rule.stateReason.slice('replaced_by:'.length)
    : null
  return (
    <div className="rounded-9 border border-ledger-border bg-card px-4 py-3">
      <div className="flex items-start justify-between gap-4">
        <div className="min-w-0">
          <div className="flex min-w-0 items-center gap-2">
            <span className="shrink-0 font-mono font-semibold text-[11.5px] text-cyanotype-blue">
              {rule.method ?? 'ANY'}
            </span>
            <span className="truncate font-mono text-[12.5px] text-ink">
              {rule.pathname ?? rule.id}
            </span>
          </div>
          <div className="mt-1 truncate text-[11.5px] text-ink-3">
            {rule.origin ?? ''} ·{' '}
            {rule.scope === 'all_tabs' ? '所有标签页' : '仅来源标签页'} ·{' '}
            {rule.state === 'active'
              ? '生效中'
              : rule.state === 'failed'
                ? `生效失败（${rule.stateReason ?? '未知原因'}）`
                : rule.state === 'applying'
                  ? '正在生效…'
                  : '已停用'}
          </div>
        </div>
        <div className="flex shrink-0 items-center gap-2">
          <Switch
            aria-label={`启用规则 ${rule.method ?? ''} ${rule.pathname ?? ''}`}
            checked={rule.enabled}
            disabled={busy}
            onCheckedChange={(checked) => onToggle(Boolean(checked))}
          />
          <DropdownMenu>
            <DropdownMenuTrigger
              render={
                <Button size="icon-xs" variant="ghost" aria-label="规则操作" />
              }
            >
              <MoreHorizontal className="size-3.5" />
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end">
              <DropdownMenuItem onClick={onFixMatch}>
                检查匹配并忽略差异
              </DropdownMenuItem>
              <DropdownMenuItem className="text-red-500" onClick={onDelete}>
                删除规则
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </div>
      </div>
      <div className="mt-2 flex flex-wrap items-center gap-x-4 gap-y-1 text-[11.5px] text-ink-2">
        <span>
          命中{' '}
          {rule.lastHit
            ? `${formatTime(rule.lastHit.at)} · ${rule.lastHit.outcome}`
            : '等待中 · 尚未出现匹配请求'}
        </span>
        {rule.ignoreKeys && rule.ignoreKeys.length > 0 ? (
          <span>忽略 query: {rule.ignoreKeys.join(', ')}</span>
        ) : null}
        <span>{rule.strictQuery ? 'strict query' : '宽松 query'}</span>
        <span>v{rule.currentRevision}</span>
        {rule.authorClient ? <span>by {rule.authorClient}</span> : null}
        <button
          type="button"
          className="text-ink-3 underline decoration-border-2 underline-offset-2 hover:text-ink"
          onClick={() => setExpanded((value) => !value)}
        >
          {expanded ? '收起详情' : '详情 / 版本'}
        </button>
      </div>
      {expanded ? (
        <RuleExpanded
          rule={rule}
          currentTabId={currentTabId}
          onRefresh={onRefresh}
        />
      ) : null}
      {rule.stateReason === 'unbound' ? (
        <p className="mt-2 text-[11.5px] text-amber-700 dark:text-amber-400">
          绑定的标签页已关闭，规则已停用；重新 apply 可绑定新 tab。
        </p>
      ) : null}
      {replacedBy ? (
        <div className="mt-2 flex items-center gap-2 text-[11.5px] text-amber-700 dark:text-amber-400">
          <span>已被规则 {replacedBy} 替换</span>
          <Button
            size="xs"
            variant="outline"
            disabled={busy}
            onClick={() => onToggle(true)}
          >
            恢复并替换
          </Button>
        </div>
      ) : null}
      {rule.lastHit?.outcome !== 'fulfilled' ? (
        <p className="mt-1.5 text-[11px] text-ink-3">
          若页面未变，可能被缓存或 Service Worker 应答；尝试硬刷新后再操作。
        </p>
      ) : null}
    </div>
  )
}

function RuleExpanded({
  rule,
  currentTabId,
  onRefresh,
}: {
  rule: NetworkRule
  currentTabId?: number
  onRefresh: () => void
}) {
  const [detail, setDetail] = useState<RuleDetail | null>(null)
  const [revisions, setRevisions] = useState<RuleRevision[]>([])
  const [test, setTest] = useState<{
    matched: number
    considered: number
  } | null>(null)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    let alive = true
    Promise.all([
      networkApi.ruleDetail(rule.id),
      networkApi.ruleRevisions(rule.id),
    ])
      .then(([nextDetail, nextRevisions]) => {
        if (!alive) return
        setDetail(nextDetail)
        setRevisions(nextRevisions.revisions)
      })
      .catch((cause) => {
        if (alive) setError(readError(cause))
      })
    return () => {
      alive = false
    }
  }, [rule.id])

  const current = detail?.revision
  const previous = revisions.find(
    (revision) => revision.revision === rule.currentRevision - 1,
  )
  return (
    <div className="mt-3 flex flex-col gap-3 border-ledger-divider border-t pt-3">
      {error ? <p className="text-[12px] text-red-500">{error}</p> : null}
      {current ? (
        <dl className="grid grid-cols-[92px_minmax(0,1fr)] gap-x-3 gap-y-1 text-[11.5px] leading-5">
          <dt className="text-ink-3">Matcher</dt>
          <dd className="break-all font-mono text-ink-2">
            {current.matcher.method} {current.matcher.origin}
            {current.matcher.pathname}
            {current.matcher.exactUrl ? ' · exact URL' : ''}
            {current.matcher.bodyFields?.length
              ? ` · ${current.matcher.bodyFields.length} bodyFields`
              : ''}
          </dd>
          <dt className="text-ink-3">Query</dt>
          <dd className="text-ink-2">
            {current.matcher.queryPolicy?.mode === 'strict'
              ? 'strict'
              : 'lenient'}
            {current.matcher.queryPolicy?.ignoreKeys?.length
              ? ` · ignore ${current.matcher.queryPolicy?.ignoreKeys?.join(', ')}`
              : ''}
          </dd>
          <dt className="text-ink-3">Hits</dt>
          <dd className="text-ink-2">{detail?.hitCount ?? 0}</dd>
          <dt className="text-ink-3">Response</dt>
          <dd className="text-ink-2">
            {current.response.status ?? 200} · delay{' '}
            {current.response.delayMs ?? 0}ms
          </dd>
          <dt className="text-ink-3">Source</dt>
          <dd className="break-all font-mono text-ink-2">
            {current.source?.handoffId ?? '—'}
            {current.source?.recordId ? ` · ${current.source.recordId}` : ''}
          </dd>
          <dt className="text-ink-3">Diff</dt>
          <dd className="text-ink-2">
            {previous
              ? `v${previous.revision} → v${current.revision} · ${
                  previous.matcher.queryPolicy?.ignoreKeys?.join(',') ===
                  current.matcher.queryPolicy?.ignoreKeys?.join(',')
                    ? 'matcher unchanged'
                    : 'ignoreKeys changed'
                }`
              : 'initial revision'}
          </dd>
        </dl>
      ) : (
        <p className="text-[12px] text-ink-3">读取规则详情…</p>
      )}
      {detail?.bodyPreview ? (
        <div>
          <div className="mb-1 font-semibold text-[10.5px] text-ink-3 uppercase tracking-[0.05em]">
            Response preview
          </div>
          <pre className="max-h-36 overflow-auto whitespace-pre-wrap break-all rounded-md border border-ledger-divider bg-card-tint/40 p-2 font-mono text-[11px] text-ink-2 leading-5">
            {detail.bodyPreview.encoding === 'base64'
              ? '二进制 body（base64 已省略）'
              : detail.bodyPreview.text}
          </pre>
        </div>
      ) : null}
      <div className="flex flex-wrap items-center gap-2">
        <Button
          size="xs"
          variant="outline"
          onClick={() => {
            void networkApi
              .testRule(rule.id)
              .then((result) =>
                setTest({
                  matched: result.matched,
                  considered: result.considered,
                }),
              )
              .catch((cause) => setError(readError(cause)))
          }}
        >
          在历史样本上测试
        </Button>
        {test ? (
          <span className="text-[11.5px] text-ink-2">
            {test.matched} / {test.considered} 命中
          </span>
        ) : null}
        <Button
          size="xs"
          variant="outline"
          disabled={rule.scope === 'all_tabs'}
          onClick={() => {
            void networkApi
              .setRuleScope(rule.id, 'all_tabs')
              .then(onRefresh)
              .catch((cause) => setError(readError(cause)))
          }}
        >
          所有标签页
        </Button>
        <Button
          size="xs"
          variant="outline"
          disabled={!currentTabId || rule.scope === 'source_tab'}
          onClick={() => {
            if (!currentTabId) return
            void networkApi
              .setRuleScope(rule.id, 'tab', currentTabId)
              .then(onRefresh)
              .catch((cause) => setError(readError(cause)))
          }}
        >
          仅当前 tab
        </Button>
      </div>
      <div>
        <div className="mb-1 font-semibold text-[10.5px] text-ink-3 uppercase tracking-[0.05em]">
          Revisions
        </div>
        <div className="flex flex-col gap-1">
          {revisions.map((revision) => (
            <div
              key={revision.revision}
              className="flex items-center justify-between gap-2 rounded border border-ledger-divider px-2 py-1 text-[11.5px]"
            >
              <span className="text-ink-2">
                v{revision.revision} · {formatTime(revision.createdAt)} ·{' '}
                {revision.matcher.queryPolicy?.ignoreKeys?.join(', ') ||
                  'no ignoreKeys'}
              </span>
              {revision.revision !== rule.currentRevision ? (
                <Button
                  size="xs"
                  variant="ghost"
                  onClick={() => {
                    void networkApi
                      .restoreRuleRevision(rule.id, revision.revision)
                      .then(onRefresh)
                      .catch((cause) => setError(readError(cause)))
                  }}
                >
                  回退到此版本
                </Button>
              ) : (
                <span className="text-ink-3">当前</span>
              )}
            </div>
          ))}
        </div>
      </div>
    </div>
  )
}

function readError(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause)
}

function splitUrl(url: string): { host: string; path: string } {
  try {
    const parsed = new URL(url)
    return { host: parsed.origin, path: `${parsed.pathname}${parsed.search}` }
  } catch {
    return { host: '', path: url }
  }
}

function formatRelative(ms: number): string {
  const seconds = Math.max(0, Math.floor((Date.now() - ms) / 1000))
  if (seconds < 5) return 'now'
  if (seconds < 60) return `${seconds}s`
  const minutes = Math.floor(seconds / 60)
  if (minutes < 60) return `${minutes}m`
  const hours = Math.floor(minutes / 60)
  if (hours < 24) return `${hours}h`
  return formatTime(ms)
}

function formatDuration(ms: number): string {
  if (ms < 1000) return `${Math.max(1, Math.round(ms))}ms`
  return `${(ms / 1000).toFixed(1)}s`
}

function formatTime(ms: number): string {
  return new Date(ms).toLocaleTimeString([], {
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
  })
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`
}

function prettify(value: string): string {
  const trimmed = value.trim()
  if (!trimmed.startsWith('{') && !trimmed.startsWith('[')) return value
  try {
    return JSON.stringify(JSON.parse(trimmed), null, 2)
  } catch {
    return value
  }
}
