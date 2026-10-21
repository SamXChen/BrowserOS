import type { NetworkCaptureTab, NetworkRule } from '@browseros/claw-api'
import { MoreHorizontal, Search, Trash2 } from 'lucide-react'
import { RuleCard } from '@/components/network/RuleCard'
import { RuleDetail } from '@/components/network/RuleDetail'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { EditorialEmpty } from '@/components/ui/EditorialEmpty'
import { Skeleton } from '@/components/ui/skeleton'
import { cn } from '@/lib/utils'
import { networkCopy } from '@/screens/network/network.copy'
import type { RuleStateFilter } from '@/screens/network/network.search-params'

const copy = networkCopy.rules

export function matchesRuleState(
  rule: NetworkRule,
  filter: RuleStateFilter,
): boolean {
  switch (filter) {
    case 'active':
      return rule.state === 'active'
    case 'pending':
      return rule.state === 'pending'
    case 'attention':
      return rule.state === 'unbound' || rule.state === 'failed'
    case 'disabled':
      return rule.state === 'disabled'
    default:
      return true
  }
}

export function filterRules(
  rules: NetworkRule[],
  query: string,
  filter: RuleStateFilter,
): NetworkRule[] {
  const tokens = query.trim().toLowerCase().split(/\s+/).filter(Boolean)
  return rules.filter((rule) => {
    if (!matchesRuleState(rule, filter)) return false
    if (tokens.length === 0) return true
    const haystack = `${rule.name ?? ''} ${rule.id} ${
      rule.method ?? rule.matcher?.method ?? ''
    } ${rule.origin ?? rule.matcher?.origin ?? ''}${
      rule.pathname ?? rule.matcher?.pathname ?? ''
    }`.toLowerCase()
    return tokens.every((token) => haystack.includes(token))
  })
}

function counts(rules: NetworkRule[]): Record<RuleStateFilter, number> {
  return {
    all: rules.length,
    active: rules.filter((rule) => rule.state === 'active').length,
    pending: rules.filter((rule) => rule.state === 'pending').length,
    attention: rules.filter(
      (rule) => rule.state === 'unbound' || rule.state === 'failed',
    ).length,
    disabled: rules.filter((rule) => rule.state === 'disabled').length,
  }
}

export interface RulesPanelProps {
  rules: NetworkRule[]
  rulesById: Map<string, NetworkRule>
  loading: boolean
  tabs: NetworkCaptureTab[]
  search: string
  stateFilter: RuleStateFilter
  expandedRuleId: string | null
  pendingRuleId: string | null
  now: number
  onSearch(value: string): void
  onStateFilter(value: RuleStateFilter): void
  onExpand(id: string | null): void
  onToggle(rule: NetworkRule, enabled: boolean): void
  onRename(rule: NetworkRule, name: string): Promise<unknown>
  onDelete(rule: NetworkRule): void
  onDeleteAll(): void
  onCopyForAi(rule: NetworkRule): void
  onReload(rule: NetworkRule, ignoreCache: boolean): void
  onRebind(rule: NetworkRule, tabId: number): void
  onScopeAllTabs(rule: NetworkRule): void
  onRevert(rule: NetworkRule, revision: number): Promise<unknown>
  onTest(rule: NetworkRule): Promise<unknown>
}

export function RulesPanel(props: RulesPanelProps) {
  const { rules, loading, search, stateFilter } = props
  const chips = counts(rules)
  const visible = filterRules(rules, search, stateFilter)

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-2">
      <div className="flex flex-wrap items-center gap-2">
        <div className="relative min-w-[200px] max-w-xs flex-1">
          <Search
            aria-hidden
            className="absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2 text-ink-3"
          />
          <input
            aria-label={copy.searchPlaceholder}
            className="h-8 w-full rounded-md border border-ledger-border bg-background pr-3 pl-8 text-[12px]"
            onChange={(event) => props.onSearch(event.target.value)}
            placeholder={copy.searchPlaceholder}
            type="search"
            value={search}
          />
        </div>
        <div className="flex flex-wrap items-center gap-1.5">
          {(['all', 'active', 'pending', 'attention', 'disabled'] as const).map(
            (filter) => (
              <button
                aria-pressed={stateFilter === filter}
                className={cn(
                  'rounded-full px-2.5 py-1 text-[11.5px]',
                  stateFilter === filter
                    ? 'bg-card-tint text-ink'
                    : 'text-ink-3 hover:text-ink-2',
                )}
                key={filter}
                onClick={() => props.onStateFilter(filter)}
                type="button"
              >
                {copy.chips[filter](chips[filter])}
              </button>
            ),
          )}
        </div>
        <DropdownMenu>
          <DropdownMenuTrigger
            render={
              <Button
                aria-label={copy.menu.listTrigger}
                size="icon-xs"
                variant="ghost"
              />
            }
          >
            <MoreHorizontal className="size-3.5" />
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end">
            <DropdownMenuItem
              disabled={rules.length === 0}
              onClick={props.onDeleteAll}
              variant="destructive"
            >
              <Trash2 className="size-3.5" />
              {copy.menu.deleteAll}
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      </div>

      <div className="min-h-0 flex-1 overflow-auto">
        {loading ? (
          <div className="flex flex-col gap-2">
            {['first', 'second', 'third'].map((row) => (
              <Skeleton className="h-20 w-full" key={row} />
            ))}
          </div>
        ) : visible.length === 0 ? (
          <div className="flex h-full items-center justify-center">
            <EditorialEmpty {...copy.empty} />
          </div>
        ) : (
          <div className="flex flex-col gap-2">
            {visible.map((rule) => (
              <RuleCard
                expanded={props.expandedRuleId === rule.id}
                key={rule.id}
                now={props.now}
                onCopyForAi={() => props.onCopyForAi(rule)}
                onDelete={() => props.onDelete(rule)}
                onExpand={() =>
                  props.onExpand(
                    props.expandedRuleId === rule.id ? null : rule.id,
                  )
                }
                onRebind={(tabId) => props.onRebind(rule, tabId)}
                onReload={(ignoreCache) => props.onReload(rule, ignoreCache)}
                onRename={(name) => props.onRename(rule, name)}
                onScopeAllTabs={() => props.onScopeAllTabs(rule)}
                onToggle={(enabled) => props.onToggle(rule, enabled)}
                pending={props.pendingRuleId === rule.id}
                rule={rule}
                rulesById={props.rulesById}
                tabs={props.tabs}
              >
                <RuleDetail
                  onRevert={(revision) => props.onRevert(rule, revision)}
                  onTest={() => props.onTest(rule) as Promise<never>}
                  rule={rule}
                />
              </RuleCard>
            ))}
          </div>
        )}
      </div>
    </div>
  )
}
