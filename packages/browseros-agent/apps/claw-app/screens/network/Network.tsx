import { useEffect, useMemo, useRef, useState } from 'react'
import { useSearchParams } from 'react-router'
import { toast } from 'sonner'
import { CaptureHeader } from '@/components/network/CaptureHeader'
import { ConnectionBanner } from '@/components/network/ConnectionBanner'
import {
  ClearRecordsDialog,
  DeleteAllRulesDialog,
} from '@/components/network/NetworkDialogs'
import { NetworkTabsHeader } from '@/components/network/NetworkTabsHeader'
import { RequestsTab } from '@/components/network/RequestsTab'
import { RulesTab } from '@/components/network/RulesTab'
import { Alert, AlertDescription } from '@/components/ui/alert'
import { Button } from '@/components/ui/button'
import { Tabs, TabsContent } from '@/components/ui/tabs'
import { TooltipProvider } from '@/components/ui/tooltip'
import { useNetworkActions } from './network.actions'
import { networkCopy } from './network.copy'
import { useNetworkScreenData } from './network.data'
import { DEFAULT_FILTERS, filterRecords } from './network.helpers'
import {
  type NetworkSearchState,
  parseNetworkSearch,
  toNetworkSearch,
} from './network.search-params'

const copy = networkCopy
const errorText = (cause: unknown) =>
  cause instanceof Error ? cause.message : String(cause)

function useFilterOptions(
  records: ReturnType<typeof useNetworkScreenData>['records'],
) {
  return useMemo(
    () => ({
      types: Array.from(
        new Set(
          records
            .map((record) => record.resourceType)
            .filter((value): value is string => Boolean(value)),
        ),
      ).sort(),
      tabs: Array.from(
        new Set(
          records
            .map((record) => record.tabTitle)
            .filter((value): value is string => Boolean(value)),
        ),
      ).sort(),
    }),
    [records],
  )
}

export function Network() {
  const data = useNetworkScreenData()
  const actions = useNetworkActions(data)
  const [params, setParams] = useSearchParams()
  const state = parseNetworkSearch(params)
  const [rulesQuery, setRulesQuery] = useState('')
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const [confirmClear, setConfirmClear] = useState(false)
  const [confirmDeleteAll, setConfirmDeleteAll] = useState(false)
  const [pendingRuleId, setPendingRuleId] = useState<string | null>(null)
  const listRef = useRef<HTMLDivElement>(null)
  const everConnected = useRef(false)

  // Typing replaces the entry; view and facet changes push one, so back
  // restores the previous tab and filters. The updater reads the live params,
  // because a streamed record can re-render this component mid-debounce.
  const update = (
    patch: Partial<NetworkSearchState>,
    replace = false,
  ): void => {
    setParams(
      (current) =>
        toNetworkSearch({ ...parseNetworkSearch(current), ...patch }),
      { replace },
    )
  }

  useEffect(() => {
    // The first connection is not a recovery; only say so after a real drop.
    if (data.connection === 'open') {
      if (everConnected.current) toast(copy.connection.restored)
      everConnected.current = true
    }
  }, [data.connection])

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
      document.querySelector<HTMLInputElement>('input[type=search]')?.focus()
    }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [])

  const records = data.records
  const options = useFilterOptions(records)
  const visibleRecords = useMemo(
    () => filterRecords(records, { ...DEFAULT_FILTERS, ...state }),
    [records, state],
  )
  const rules = data.rules
  const selected =
    records.find((record) => record.recordId === selectedId) ?? null
  const ruleNames = new Map(
    rules.map((rule) => [rule.id, rule.name ?? rule.id]),
  )
  const countByState = (states: string[]): number =>
    rules.filter((rule) => states.includes(rule.state)).length
  const running = data.status?.capture.running ?? false
  const paused = running && data.status?.browser.connected === false
  const recordCount = data.status?.capture.records ?? records.length

  return (
    <TooltipProvider>
      <div className="relative mx-auto flex w-full max-w-7xl flex-col gap-4 px-6 pt-8 pb-16 lg:h-screen lg:overflow-hidden lg:px-8 lg:pt-6 lg:pb-6">
        <CaptureHeader
          activeRules={countByState(['active'])}
          hasRecords={records.length > 0}
          onClear={() => setConfirmClear(true)}
          onStart={actions.startCapture}
          onStop={actions.stopCapture}
          onStopAll={actions.stopAll}
          paused={paused}
          recordCount={recordCount}
          running={running}
          statusPending={data.statusPending}
        />

        <ConnectionBanner state={data.connection} />

        {data.statusError ? (
          <Alert>
            <AlertDescription className="flex items-center gap-2">
              {copy.errors.status(errorText(data.statusError))}
              <Button onClick={data.refetchStatus} size="xs" variant="outline">
                {copy.errors.retry}
              </Button>
            </AlertDescription>
          </Alert>
        ) : null}

        <Tabs
          className="flex min-h-0 flex-1 flex-col gap-3"
          onValueChange={(value) =>
            update({ view: value === 'rules' ? 'rules' : 'requests' })
          }
          value={state.view}
        >
          <NetworkTabsHeader
            activeRules={countByState(['active'])}
            attentionRules={countByState(['unbound', 'failed'])}
            gapEvents={data.status?.capture.gapEvents ?? 0}
            onAttention={() =>
              update({ view: 'rules', ruleState: 'attention' })
            }
            pendingRules={countByState(['pending'])}
            requests={visibleRecords.length}
            rules={rules.length}
            runtime={data.status?.runtime}
          />

          <TabsContent
            className="flex min-h-0 flex-1 flex-col"
            value="requests"
          >
            <RequestsTab
              listRef={listRef}
              loading={data.statusPending}
              onCopy={actions.copyReference}
              onDelete={(id) =>
                actions.deleteRecord(id, () =>
                  setSelectedId((current) => (current === id ? null : current)),
                )
              }
              onOpenRule={(ruleId) => update({ view: 'rules', rule: ruleId })}
              onSelect={setSelectedId}
              records={records}
              ruleNames={ruleNames}
              selected={selected}
              selectedId={selectedId}
              state={state}
              tabOptions={options.tabs}
              total={recordCount}
              typeOptions={options.types}
              update={update}
              visibleRecords={visibleRecords}
            />
          </TabsContent>

          <TabsContent className="flex min-h-0 flex-1 flex-col" value="rules">
            <RulesTab
              data={data}
              onCopyForAi={actions.copyForAi}
              onDeleteAll={() => setConfirmDeleteAll(true)}
              onPending={setPendingRuleId}
              onSearch={setRulesQuery}
              pendingRuleId={pendingRuleId}
              search={rulesQuery}
              state={state}
              update={update}
            />
          </TabsContent>
        </Tabs>

        <ClearRecordsDialog
          count={records.length}
          onConfirm={() => {
            setConfirmClear(false)
            actions.clearRecords(() => setSelectedId(null))
          }}
          onOpenChange={setConfirmClear}
          open={confirmClear}
        />
        <DeleteAllRulesDialog
          count={rules.length}
          onConfirm={() => {
            setConfirmDeleteAll(false)
            actions.deleteAllRules()
          }}
          onOpenChange={setConfirmDeleteAll}
          open={confirmDeleteAll}
        />
      </div>
    </TooltipProvider>
  )
}
