import { Ban, X } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { cn } from '@/lib/utils'
import { networkCopy } from '@/screens/network/network.copy'

const copy = networkCopy

export function CaptureHeader({
  statusPending,
  running,
  paused,
  recordCount,
  activeRules,
  hasRecords,
  onStart,
  onStop,
  onStopAll,
  onClear,
}: {
  statusPending: boolean
  running: boolean
  paused: boolean
  recordCount: number
  activeRules: number
  hasRecords: boolean
  onStart(): void
  onStop(): void
  onStopAll(): void
  onClear(): void
}) {
  const label = statusPending
    ? copy.capture.connecting
    : paused
      ? copy.capture.paused
      : running
        ? copy.capture.running(recordCount)
        : hasRecords
          ? copy.capture.stoppedKeeping(recordCount)
          : copy.capture.stopped

  return (
    <header className="flex flex-wrap items-start justify-between gap-4">
      <div className="flex max-w-2xl flex-col gap-1.5">
        <h1 className="font-extrabold text-3xl leading-tight tracking-tight md:text-4xl">
          {copy.page.title}
        </h1>
        <p className="text-[13.5px] text-ink-2 leading-relaxed">
          {copy.page.description}
        </p>
      </div>
      <div className="flex flex-wrap items-center gap-2">
        <span
          className={cn(
            'inline-flex items-center gap-1.5 rounded-full px-2.5 py-[4px] font-medium text-[12px]',
            !statusPending && paused
              ? 'bg-amber-500/15 text-amber-700 dark:text-amber-400'
              : !statusPending && running
                ? 'bg-cyanotype-live text-cyanotype-live-ink'
                : 'bg-card-tint text-ink-2',
          )}
        >
          <span
            className={cn(
              'size-1.5 rounded-full',
              !statusPending && paused
                ? 'bg-amber-500'
                : !statusPending && running
                  ? 'bg-cyanotype-live-ink'
                  : 'bg-ink-3',
            )}
          />
          {label}
        </span>
        <Button
          disabled={statusPending}
          onClick={running ? onStop : onStart}
          size="sm"
          variant={running ? 'outline' : 'default'}
        >
          {running ? copy.capture.stop : copy.capture.start}
        </Button>
        <Button
          disabled={activeRules === 0}
          onClick={onStopAll}
          size="sm"
          variant="outline"
        >
          <Ban className="size-3.5" />
          {copy.capture.stopAll}
        </Button>
        {hasRecords ? (
          <Button onClick={onClear} size="sm" variant="ghost">
            <X className="size-3.5" />
            {copy.capture.clear}
          </Button>
        ) : null}
      </div>
    </header>
  )
}
