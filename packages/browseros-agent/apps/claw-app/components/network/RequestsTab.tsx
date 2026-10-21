import type { RefObject } from 'react'
import { RequestDetail } from '@/components/network/RequestDetail'
import { RequestFilters } from '@/components/network/RequestFilters'
import { RequestList } from '@/components/network/RequestList'
import type { DerivedRecord } from '@/screens/network/network.helpers'
import type {
  NetworkSearchState,
  RuleStateFilter,
} from '@/screens/network/network.search-params'

export function RequestsTab({
  state,
  update,
  records,
  visibleRecords,
  selected,
  selectedId,
  ruleNames,
  typeOptions,
  tabOptions,
  loading,
  total,
  listRef,
  onSelect,
  onCopy,
  onDelete,
  onOpenRule,
}: {
  state: NetworkSearchState
  update(patch: Partial<NetworkSearchState>, replace?: boolean): void
  records: DerivedRecord[]
  visibleRecords: DerivedRecord[]
  selected: DerivedRecord | null
  selectedId: string | null
  ruleNames: Map<string, string>
  typeOptions: string[]
  tabOptions: string[]
  loading: boolean
  total: number
  listRef: RefObject<HTMLDivElement | null>
  onSelect(id: string): void
  onCopy(id: string): void
  onDelete(id: string): void
  onOpenRule(id: string): void
}) {
  return (
    <>
      <RequestFilters
        collectorCount={
          records.filter((record) => record.derived?.collector).length
        }
        onChange={(patch) => update(patch, 'query' in patch)}
        onReset={() =>
          update({
            query: '',
            method: 'all',
            status: 'all',
            resourceType: 'all',
            tab: 'all',
            mock: false,
            body: false,
            hideAgent: true,
            ruleState: 'all' as RuleStateFilter,
          })
        }
        state={state}
        tabOptions={tabOptions}
        typeOptions={typeOptions}
      />
      <div className="grid min-h-0 flex-1 gap-3 lg:grid-cols-[minmax(0,1fr)_minmax(0,420px)]">
        <RequestList
          filtered={visibleRecords.length}
          listRef={listRef}
          loading={loading}
          onCopy={onCopy}
          onDelete={onDelete}
          onOpenRule={onOpenRule}
          onSelect={onSelect}
          records={visibleRecords}
          renderLimit={500}
          ruleNames={ruleNames}
          selectedId={selectedId}
          total={total}
        />
        <aside className="min-h-0 overflow-hidden rounded-9 border border-ledger-border bg-card">
          <RequestDetail
            onOpenRule={onOpenRule}
            record={selected}
            ruleName={
              selected?.mock ? ruleNames.get(selected.mock.ruleId) : undefined
            }
          />
        </aside>
      </div>
    </>
  )
}
