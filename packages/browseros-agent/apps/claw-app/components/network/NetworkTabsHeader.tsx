import { TabsList, TabsTrigger } from '@/components/ui/tabs'
import { networkCopy } from '@/screens/network/network.copy'

const copy = networkCopy

const TAB_TRIGGER =
  'h-7 flex-none rounded-md border-0 px-3 font-normal text-[12px] text-cyanotype-muted data-active:bg-white data-active:font-semibold data-active:text-cyanotype-blue'

export function NetworkTabsHeader({
  requests,
  rules,
  activeRules,
  pendingRules,
  attentionRules,
  gapEvents,
  runtime,
  onAttention,
}: {
  requests: number
  rules: number
  activeRules: number
  pendingRules: number
  attentionRules: number
  gapEvents: number
  runtime?: {
    paused?: number
    decisionP95Ms?: number
    rulesGeneration?: number
    processorP95Ms?: number
    processorInflight?: number
  }
  onAttention(): void
}) {
  return (
    <div className="flex shrink-0 flex-wrap items-center justify-between gap-3">
      <TabsList
        aria-label={copy.views.label}
        className="h-9 rounded-[9px] bg-cyanotype-well p-1"
      >
        <TabsTrigger className={TAB_TRIGGER} value="requests">
          {copy.views.requestsCount(requests)}
        </TabsTrigger>
        <TabsTrigger className={TAB_TRIGGER} value="rules">
          {copy.views.rulesCount(rules)}
        </TabsTrigger>
      </TabsList>
      <div className="flex items-center gap-2 text-[12px] text-ink-3">
        <span className="rounded-full bg-card-tint px-2.5 py-1 text-[11.5px] text-ink-2">
          {copy.header.mockActive(activeRules)}
          {pendingRules > 0 ? copy.header.mockPending(pendingRules) : ''}
          {attentionRules > 0 ? (
            <button
              className="text-red-600 dark:text-red-400"
              onClick={onAttention}
              type="button"
            >
              {copy.header.mockAttention(attentionRules)}
            </button>
          ) : null}
        </span>
        {gapEvents > 0 ? (
          <span className="text-amber-600">{copy.header.gap(gapEvents)}</span>
        ) : null}
        {(runtime?.paused ?? 0) > 0 ||
        (runtime?.decisionP95Ms ?? 0) > 100 ||
        (runtime?.processorP95Ms ?? 0) > 100 ? (
          <span
            className="font-mono text-[10.5px]"
            title={copy.header.queueTitle(runtime?.rulesGeneration ?? 0)}
          >
            {copy.header.queued(runtime?.paused ?? 0)} ·{' '}
            {copy.header.p95(runtime?.decisionP95Ms ?? 0)}
            {(runtime?.processorP95Ms ?? 0) > 100
              ? ` · ${copy.header.processorP95(runtime?.processorP95Ms ?? 0)}`
              : ''}
          </span>
        ) : null}
      </div>
    </div>
  )
}
