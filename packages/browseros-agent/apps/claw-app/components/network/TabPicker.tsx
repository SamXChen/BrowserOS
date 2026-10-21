import type { NetworkCaptureTab } from '@browseros/claw-api'
import type { ReactNode } from 'react'
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from '@/components/ui/popover'
import { networkCopy } from '@/screens/network/network.copy'

/** NEOB and extension pages cannot be mocked, so they never appear (U16). */
export function pickableTabs(
  tabs: NetworkCaptureTab[],
  sourceTabId?: number,
): NetworkCaptureTab[] {
  const pickable = tabs.filter(
    (tab) => tab.title !== 'NEOB' && !tab.url.startsWith('chrome-extension://'),
  )
  return pickable.slice().sort((left, right) => {
    if (left.tabId === sourceTabId) return -1
    if (right.tabId === sourceTabId) return 1
    return 0
  })
}

export function TabPicker({
  tabs,
  sourceTabId,
  onSelect,
  children,
}: {
  tabs: NetworkCaptureTab[]
  sourceTabId?: number
  onSelect(tabId: number): void
  children: ReactNode
}) {
  const options = pickableTabs(tabs, sourceTabId)
  return (
    <Popover>
      <PopoverTrigger render={children as never} />
      <PopoverContent align="start" className="w-80 p-1">
        {options.length === 0 ? (
          <p className="px-2 py-3 text-[12px] text-ink-3">
            {networkCopy.filters.allTabs}
          </p>
        ) : (
          <ul className="flex max-h-80 flex-col overflow-auto">
            {options.map((tab) => (
              <li key={tab.tabId}>
                <button
                  className="flex w-full flex-col gap-0.5 rounded px-2 py-1.5 text-left hover:bg-card-tint"
                  onClick={() => onSelect(tab.tabId)}
                  type="button"
                >
                  <span className="flex items-center gap-1.5 text-[12.5px] text-ink">
                    {tab.title || tab.url}
                    {tab.tabId === sourceTabId ? (
                      <span className="rounded bg-cyanotype-blue/10 px-1 text-[10px] text-cyanotype-blue">
                        {networkCopy.rules.detail.sourceTab}
                      </span>
                    ) : null}
                  </span>
                  <span className="truncate font-mono text-[10.5px] text-ink-3">
                    {tab.url}
                  </span>
                </button>
              </li>
            ))}
          </ul>
        )}
      </PopoverContent>
    </Popover>
  )
}
