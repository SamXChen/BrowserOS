import { Copy, Trash2 } from 'lucide-react'
import { memo } from 'react'
import { MockBadge } from '@/components/network/MockBadge'
import { cn } from '@/lib/utils'
import { networkCopy } from '@/screens/network/network.copy'
import {
  type DerivedRecord,
  formatBytes,
  formatTime,
  methodTone,
  statusTone,
} from '@/screens/network/network.helpers'

export const ROW_GRID =
  'grid grid-cols-[58px_minmax(0,1fr)_46px_62px_90px_78px] items-center gap-3'

export interface RequestRowProps {
  record: DerivedRecord
  selected: boolean
  ruleName?: string
  onSelect(recordId: string): void
  onCopy(recordId: string): void
  onDelete(recordId: string): void
  onOpenRule(ruleId: string): void
}

export const RequestRow = memo(function RequestRow({
  record,
  selected,
  ruleName,
  onSelect,
  onCopy,
  onDelete,
  onOpenRule,
}: RequestRowProps) {
  const derived = record.derived
  const failed = record.state === 'failed'
  return (
    <div
      aria-selected={selected}
      className={cn(
        ROW_GRID,
        'group cursor-pointer rounded-md px-2 py-1.5 text-[12px]',
        selected ? 'bg-cyanotype-well' : 'hover:bg-card-tint/60',
      )}
      onClick={() => onSelect(record.recordId)}
      onKeyDown={(event) => {
        if (event.key === 'Enter' || event.key === ' ') {
          event.preventDefault()
          onSelect(record.recordId)
        }
      }}
      role="option"
      tabIndex={-1}
    >
      <span className="font-mono text-[11px] text-ink-3">
        {formatTime(record.startedAtMs)}
      </span>
      <span className="flex min-w-0 flex-col gap-0.5">
        <span className="flex min-w-0 items-center gap-1.5">
          <span
            className={cn(
              'shrink-0 rounded px-1 py-0.5 font-semibold text-[10px]',
              methodTone(record.method),
            )}
          >
            {record.method}
          </span>
          <span className="truncate text-ink" title={record.url}>
            {derived?.path ?? record.url}
          </span>
          {record.mock ? (
            <MockBadge
              mock={record.mock}
              onOpenRule={() => onOpenRule(record.mock?.ruleId ?? '')}
              ruleName={ruleName}
              status={record.status}
            />
          ) : null}
        </span>
        <span className="truncate font-mono text-[10.5px] text-ink-3">
          {derived?.host}
          {ruleName ? ` · ${ruleName}` : ''}
        </span>
      </span>
      <span
        className={cn(
          'font-mono text-[11.5px]',
          statusTone(record.status, failed),
        )}
      >
        {record.status ?? (failed ? 'failed' : '…')}
      </span>
      <span className="font-mono text-[11px] text-ink-3">
        {record.encodedDataLength !== undefined
          ? formatBytes(record.encodedDataLength)
          : '—'}
      </span>
      <span className="truncate text-[11px] text-ink-3">
        {record.tabTitle ?? '—'}
      </span>
      <span className="flex items-center justify-end gap-1 opacity-0 group-focus-within:opacity-100 group-aria-selected:opacity-100">
        <button
          aria-label={networkCopy.list.copyReference}
          className="rounded p-1 text-ink-3 hover:bg-card-tint hover:text-ink"
          onClick={(event) => {
            event.stopPropagation()
            onCopy(record.recordId)
          }}
          type="button"
        >
          <Copy className="size-3.5" />
        </button>
        <button
          aria-label={networkCopy.list.deleteRecord}
          className="rounded p-1 text-ink-3 hover:bg-red-500/10 hover:text-red-600"
          onClick={(event) => {
            event.stopPropagation()
            onDelete(record.recordId)
          }}
          type="button"
        >
          <Trash2 className="size-3.5" />
        </button>
      </span>
    </div>
  )
})
