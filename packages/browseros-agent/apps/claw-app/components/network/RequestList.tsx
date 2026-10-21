import { type RefObject, useEffect } from 'react'
import {
  RequestRow,
  type RequestRowProps,
  ROW_GRID,
} from '@/components/network/RequestRow'
import { EditorialEmpty } from '@/components/ui/EditorialEmpty'
import { Skeleton } from '@/components/ui/skeleton'
import { cn } from '@/lib/utils'
import { networkCopy } from '@/screens/network/network.copy'
import type { DerivedRecord } from '@/screens/network/network.helpers'

/** Enter is "select", never "copy" (U7). */
export function listKeyboardReducer(
  selectedIndex: number,
  key: string,
  length: number,
): number {
  if (length === 0) return -1
  switch (key) {
    case 'ArrowDown':
      return Math.min(length - 1, selectedIndex + 1)
    case 'ArrowUp':
      return Math.max(0, selectedIndex - 1)
    case 'Home':
      return 0
    case 'End':
      return length - 1
    default:
      return selectedIndex
  }
}

const copy = networkCopy.list

const SKELETON_ROWS = [
  'one',
  'two',
  'three',
  'four',
  'five',
  'six',
  'seven',
  'eight',
]

export function RequestList({
  records,
  selectedId,
  ruleNames,
  loading,
  filtered,
  total,
  renderLimit,
  listRef,
  onSelect,
  onCopy,
  onDelete,
  onOpenRule,
}: {
  records: DerivedRecord[]
  selectedId: string | null
  ruleNames: Map<string, string>
  loading: boolean
  filtered: number
  total: number
  renderLimit: number
  listRef: RefObject<HTMLDivElement | null>
  onSelect: RequestRowProps['onSelect']
  onCopy: RequestRowProps['onCopy']
  onDelete: RequestRowProps['onDelete']
  onOpenRule: RequestRowProps['onOpenRule']
}) {
  // The first row is selected by default, so the arrow keys always have a
  // starting point (U7).
  useEffect(() => {
    if (!selectedId && records[0]) onSelect(records[0].recordId)
  }, [onSelect, records, selectedId])

  if (loading) {
    return (
      <div className="flex flex-col gap-1.5">
        {SKELETON_ROWS.map((row) => (
          <Skeleton className="h-9 w-full" key={row} />
        ))}
      </div>
    )
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="mb-1.5 flex shrink-0 items-center justify-between text-[11px] text-ink-3">
        <span>{copy.captured}</span>
        <span>
          {filtered} / {records.length}
          {total > records.length
            ? ` ${copy.renderLimitLong(renderLimit, total)}`
            : total > 0
              ? ` ${copy.renderLimit(renderLimit)}`
              : ''}
        </span>
      </div>
      <div
        className={cn(ROW_GRID, 'shrink-0 px-2 pb-1 text-[10.5px] text-ink-3')}
      >
        <span>{copy.columns.time}</span>
        <span>{copy.columns.request}</span>
        <span>{copy.columns.status}</span>
        <span>{copy.columns.size}</span>
        <span>{copy.columns.source}</span>
        <span />
      </div>
      {records.length === 0 && total === 0 ? (
        <div className="flex flex-1 items-center justify-center py-10">
          <EditorialEmpty {...copy.empty} />
        </div>
      ) : (
        <div
          aria-label={networkCopy.a11y.listLabel}
          className="min-h-0 flex-1 overflow-auto"
          onKeyDown={(event) => {
            if (!['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) {
              return
            }
            event.preventDefault()
            const index = records.findIndex(
              (record) => record.recordId === selectedId,
            )
            const next = listKeyboardReducer(index, event.key, records.length)
            const record = records[next]
            if (record) onSelect(record.recordId)
          }}
          ref={listRef}
          role="listbox"
          tabIndex={0}
        >
          {records.map((record) => (
            <RequestRow
              key={record.recordId}
              onCopy={onCopy}
              onDelete={onDelete}
              onOpenRule={onOpenRule}
              onSelect={onSelect}
              record={record}
              ruleName={
                record.mock ? ruleNames.get(record.mock.ruleId) : undefined
              }
              selected={record.recordId === selectedId}
            />
          ))}
        </div>
      )}
    </div>
  )
}
