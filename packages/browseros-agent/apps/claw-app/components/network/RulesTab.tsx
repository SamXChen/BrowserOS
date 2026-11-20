import { RulesPanel } from '@/components/network/RulesPanel'
import { toast, toastError } from '@/modules/toast/toast'
import { networkCopy } from '@/screens/network/network.copy'
import type { NetworkScreenData } from '@/screens/network/network.data'
import type {
  NetworkSearchState,
  RuleStateFilter,
} from '@/screens/network/network.search-params'

const copy = networkCopy
const errorText = (cause: unknown) =>
  cause instanceof Error ? cause.message : String(cause)

export function RulesTab({
  data,
  state,
  update,
  search,
  onSearch,
  pendingRuleId,
  onPending,
  onDeleteAll,
  onCopyForAi,
}: {
  data: NetworkScreenData
  state: NetworkSearchState
  update(patch: Partial<NetworkSearchState>): void
  search: string
  onSearch(value: string): void
  pendingRuleId: string | null
  onPending(id: string | null): void
  onDeleteAll(): void
  onCopyForAi(ruleId: string, name: string, endpoint: string): void
}) {
  const tabs = data.status?.capture.tabs ?? []
  const rules = data.rules

  const toggle = async (id: string, enabled: boolean): Promise<void> => {
    onPending(id)
    try {
      await data.actions.toggleRule(id, enabled)
    } catch (cause) {
      toastError(copy.errors.ruleAction(errorText(cause)))
    } finally {
      onPending(null)
    }
  }

  return (
    <RulesPanel
      expandedRuleId={state.rule}
      loading={data.rulesPending}
      now={Date.now()}
      onCopyForAi={(rule) =>
        onCopyForAi(
          rule.id,
          rule.name ?? rule.id,
          `${rule.origin ?? rule.matcher?.origin ?? ''}${
            rule.pathname ?? rule.matcher?.pathname ?? ''
          }`,
        )
      }
      onDelete={(rule) => {
        void data.actions.deleteRule(rule.id).then(
          () =>
            toast(copy.rules.menu.deleted, {
              action: {
                label: copy.list.undo,
                onClick: () => {
                  void data.actions.restoreRule(rule.id)
                },
              },
            }),
          (cause: unknown) => toastError(errorText(cause)),
        )
      }}
      onDeleteAll={onDeleteAll}
      onExpand={(id) => update({ rule: id })}
      onRebind={(rule, tabId) => {
        void data.actions.setScope(rule.id, 'tab', tabId)
      }}
      onReload={(rule, ignoreCache) => {
        const tabId = rule.scope === 'tab' ? rule.scopeTab : undefined
        if (tabId === undefined) {
          toastError(copy.rules.menu.tabClosedTooltip)
          return
        }
        void data.actions.reloadTab(tabId, ignoreCache).then(
          () => toast(copy.rules.menu.reloaded(String(tabId))),
          (cause: unknown) => toastError(errorText(cause)),
        )
      }}
      onRename={(rule, name) => data.actions.renameRule(rule.id, name)}
      onRevert={(rule, revision) =>
        data.actions.restoreRevision(rule.id, revision)
      }
      onScopeAllTabs={(rule) => {
        void data.actions.setScope(rule.id, 'all_tabs')
      }}
      onSearch={onSearch}
      onStateFilter={(filter: RuleStateFilter) => update({ ruleState: filter })}
      onTest={(rule) => data.actions.testRule(rule.id)}
      onToggle={(rule, enabled) => void toggle(rule.id, enabled)}
      pendingRuleId={pendingRuleId}
      rules={rules}
      rulesById={data.rulesById}
      search={search}
      stateFilter={state.ruleState}
      tabs={tabs}
    />
  )
}
