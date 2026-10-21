import type { NetworkCaptureTab, NetworkRule } from '@browseros/claw-api'
import { Copy, MoreHorizontal, RefreshCw, Trash2 } from 'lucide-react'
import type { ReactNode } from 'react'
import { RuleNameEditor } from '@/components/network/RuleNameEditor'
import { TabPicker } from '@/components/network/TabPicker'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { Spinner } from '@/components/ui/spinner'
import { Switch } from '@/components/ui/switch'
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from '@/components/ui/tooltip'
import { cn } from '@/lib/utils'
import { networkCopy } from '@/screens/network/network.copy'
import {
  formatRelative,
  ruleStateView,
} from '@/screens/network/network.helpers'

const copy = networkCopy.rules

export const CACHE_HINT_DELAY_MS = 30_000

export function shouldShowCacheHint(
  rule: NetworkRule,
  now = Date.now(),
): boolean {
  return (
    rule.state === 'active' &&
    (rule.hitCount ?? 0) === 0 &&
    now - (rule.updatedAt ?? 0) > CACHE_HINT_DELAY_MS
  )
}

export function RuleCard({
  rule,
  rulesById,
  pending,
  expanded,
  tabs,
  now,
  children,
  onToggle,
  onRename,
  onDelete,
  onExpand,
  onCopyForAi,
  onReload,
  onRebind,
  onScopeAllTabs,
}: {
  rule: NetworkRule
  rulesById: Map<string, NetworkRule>
  pending: boolean
  expanded: boolean
  tabs: NetworkCaptureTab[]
  now: number
  children?: ReactNode
  onToggle(enabled: boolean): void
  onRename(name: string): Promise<unknown>
  onDelete(): void
  onExpand(): void
  onCopyForAi(): void
  onReload(ignoreCache: boolean): void
  onRebind(tabId: number): void
  onScopeAllTabs(): void
}) {
  const state = ruleStateView(rule, (id) => rulesById.get(id)?.name)
  const lastHit = rule.lastHit
  const processorFailed =
    lastHit?.outcome === 'processor_error' ||
    lastHit?.outcome === 'processor_timeout'
  const action = rule.action
  const matcher = rule.matcher
  const sourceTabId = rule.scopeTab
  const targetTab = rule.scope === 'tab' ? sourceTabId : undefined
  const tabClosed =
    rule.state === 'unbound' ||
    (targetTab !== undefined && !tabs.some((tab) => tab.tabId === targetTab))
  const typeLabel =
    action?.kind === 'processor' ? copy.types.processor : copy.types.static
  const scopeLabel =
    rule.scope === 'tab'
      ? copy.scope.tab(
          tabs.find((tab) => tab.tabId === rule.scopeTab)?.title ??
            copy.scope.tabUnknown,
        )
      : copy.scope.allTabs
  const endpoint = matcher
    ? `${matcher.method} ${matcher.origin ?? ''}${matcher.pathname ?? ''}`
    : '—'
  const summary =
    action?.kind === 'processor'
      ? copy.summary.processor(action.timeoutMs ?? 200)
      : copy.summary.response(action?.status ?? 200, action?.delayMs ?? 0)

  return (
    <div
      className={cn(
        'flex flex-col gap-1.5 rounded-9 border bg-card px-3 py-2.5',
        expanded
          ? 'border-cyanotype-blue/40 ring-2 ring-cyanotype-blue/40'
          : 'border-ledger-border',
      )}
    >
      <div className="flex items-start gap-2">
        <div className="flex min-w-0 flex-1 items-center gap-1.5">
          <RuleNameEditor name={rule.name ?? rule.id} onRename={onRename} />
          <span
            className={cn(
              'shrink-0 rounded border px-1.5 py-0.5 font-medium text-[10.5px]',
              action?.kind === 'processor'
                ? 'border-cyanotype-blue/40 text-cyanotype-blue'
                : 'border-ledger-border text-ink-2',
            )}
          >
            {typeLabel}
          </span>
          <span
            className={cn(
              'inline-flex shrink-0 items-center gap-1.5 rounded-full px-2 py-[3px] font-medium text-[11px]',
              state.tone,
            )}
          >
            <span className={cn('size-1.5 rounded-full', state.dot)} />
            {state.label}
          </span>
        </div>
        <div className="flex shrink-0 items-center gap-1.5">
          {pending ? <Spinner className="size-3.5 text-ink-3" /> : null}
          <Switch
            aria-label={copy.actions.toggle}
            checked={rule.enabled}
            disabled={pending}
            onCheckedChange={(checked) => onToggle(Boolean(checked))}
            size="sm"
          />
          <Button
            aria-label={copy.actions.copyForAiLabel}
            onClick={onCopyForAi}
            size="xs"
            variant="outline"
          >
            <Copy className="size-3" />
            {copy.actions.copyForAi}
          </Button>
          <DropdownMenu>
            <DropdownMenuTrigger
              render={
                <Button
                  aria-label={copy.menu.triggerFor(rule.name ?? rule.id)}
                  size="icon-xs"
                  variant="ghost"
                />
              }
            >
              <MoreHorizontal className="size-3.5" />
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end">
              <DropdownMenuItem onClick={onExpand}>
                {copy.menu.details}
              </DropdownMenuItem>
              {tabClosed ? (
                <Tooltip>
                  <TooltipTrigger render={<span />}>
                    <DropdownMenuItem disabled>
                      {copy.menu.reloadTab}
                    </DropdownMenuItem>
                  </TooltipTrigger>
                  <TooltipContent>{copy.menu.tabClosedTooltip}</TooltipContent>
                </Tooltip>
              ) : (
                <>
                  <DropdownMenuItem onClick={() => onReload(false)}>
                    <RefreshCw className="size-3.5" />
                    {copy.menu.reloadTab}
                  </DropdownMenuItem>
                  <DropdownMenuItem onClick={() => onReload(true)}>
                    {copy.menu.reloadTabBypassCache}
                  </DropdownMenuItem>
                </>
              )}
              <DropdownMenuSeparator />
              <DropdownMenuItem onClick={onDelete} variant="destructive">
                <Trash2 className="size-3.5" />
                {copy.menu.delete}
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </div>
      </div>

      <button
        className="truncate text-left font-mono text-[11.5px] text-ink-3 hover:text-ink-2"
        onClick={onExpand}
        type="button"
      >
        {endpoint}
        {matcher?.exactUrl ? ` · ${copy.summary.exactUrl}` : ''}
      </button>

      <div className="flex flex-wrap items-center gap-x-2 gap-y-1 text-[11.5px] text-ink-2">
        <span>{scopeLabel}</span>
        <span>·</span>
        <span>{summary}</span>
        <span>·</span>
        <span>{copy.summary.hits(rule.hitCount ?? 0)}</span>
        <span>·</span>
        <span>
          {rule.lastHit
            ? formatRelative(rule.lastHit.at, now)
            : copy.summary.neverHit}
        </span>
        <span>·</span>
        <span>{copy.summary.revision(rule.currentRevision)}</span>
        {rule.authorClient ? (
          <>
            <span>·</span>
            <span>{copy.summary.author(rule.authorClient)}</span>
          </>
        ) : null}
      </div>

      {rule.state === 'unbound' ? (
        <div className="flex flex-wrap items-center gap-2 text-[11.5px] text-red-600 dark:text-red-400">
          <span>{copy.hint.unbound}</span>
          <TabPicker onSelect={onRebind} tabs={tabs}>
            <Button size="xs" variant="outline">
              {copy.hint.rebind}
            </Button>
          </TabPicker>
          <Button onClick={onScopeAllTabs} size="xs" variant="ghost">
            {copy.hint.allTabs}
          </Button>
        </div>
      ) : null}
      {shouldShowCacheHint(rule, now) ? (
        <p className="text-[11px] text-ink-3">{copy.hint.cache}</p>
      ) : null}
      {processorFailed ? (
        <p className="text-[11px] text-red-600 dark:text-red-400">
          {copy.hint.processorFailed(lastHit?.reason ?? '')}
        </p>
      ) : null}
      {expanded ? children : null}
    </div>
  )
}
