import type { NetworkMockMark } from '@browseros/claw-api'
import { cn } from '@/lib/utils'
import { networkCopy } from '@/screens/network/network.copy'

const copy = networkCopy.list

const BADGE = 'rounded px-1.5 py-0.5 text-[10.5px] font-medium border'

const TONE = {
  mock: 'border-ledger-border text-ink-2',
  processed: 'border-cyanotype-blue/40 text-cyanotype-blue',
  muted: 'border-ledger-border text-ink-3',
  failed: 'border-amber-500/40 text-amber-700 dark:text-amber-400',
} as const

export interface MockBadgeView {
  label: string
  tone: string
  title: string
}

/** Badge label, colour and hover text for one `NetworkMockMark`. */
export function mockBadgeView(
  mock: NetworkMockMark,
  status?: number,
  ruleName?: string,
): MockBadgeView {
  const name = ruleName ?? mock.ruleId
  switch (mock.outcome) {
    case 'processed':
      return {
        label: copy.badges.processed(status ?? '—'),
        tone: TONE.processed,
        title: copy.badgeTitles.processed(name),
      }
    case 'passthrough':
      return {
        label: copy.badges.passthrough,
        tone: TONE.muted,
        title: copy.badgeTitles.passthrough,
      }
    case 'processor_error':
    case 'processor_timeout':
      return {
        label: copy.badges.failed,
        tone: TONE.failed,
        title: copy.badgeTitles.failed(mock.reason ?? ''),
      }
    case 'processor_skipped':
      return {
        label: copy.badges.skipped,
        tone: TONE.muted,
        title: copy.badgeTitles.skipped(
          copy.skipReasons[mock.reason ?? ''] ?? mock.reason ?? '',
        ),
      }
    default:
      return {
        label: copy.badges.mock(status ?? '—'),
        tone: TONE.mock,
        title: copy.badgeTitles.mock(name),
      }
  }
}

export function MockBadge({
  mock,
  status,
  ruleName,
  onOpenRule,
}: {
  mock: NetworkMockMark
  status?: number
  ruleName?: string
  onOpenRule?: () => void
}) {
  const view = mockBadgeView(mock, status, ruleName)
  const content = (
    <span className={cn(BADGE, view.tone)} title={view.title}>
      {view.label}
    </span>
  )
  if (!onOpenRule) return content
  return (
    <button
      className="cursor-pointer"
      onClick={(event) => {
        event.stopPropagation()
        onOpenRule()
      }}
      type="button"
    >
      {content}
    </button>
  )
}
