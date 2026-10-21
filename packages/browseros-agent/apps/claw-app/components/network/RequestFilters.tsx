import { ChevronDown, Search, X } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import { cn } from '@/lib/utils'
import { networkCopy } from '@/screens/network/network.copy'
import type { RecordFacet } from '@/screens/network/network.helpers'
import { METHODS, STATUS_FACETS } from '@/screens/network/network.helpers'
import type { NetworkSearchState } from '@/screens/network/network.search-params'

const copy = networkCopy.filters

function FilterSelect({
  label,
  value,
  options,
  onChange,
}: {
  label: string
  value: string
  options: Array<{ value: string; label: string }>
  onChange(value: string): void
}) {
  return (
    <span className="relative inline-flex items-center">
      <select
        aria-label={label}
        className="h-8 appearance-none rounded-md border border-ledger-border bg-background pr-7 pl-2.5 text-[12px] text-ink-2"
        onChange={(event) => onChange(event.target.value)}
        value={value}
      >
        {options.map((option) => (
          <option key={option.value} value={option.value}>
            {option.label}
          </option>
        ))}
      </select>
      <ChevronDown
        aria-hidden
        className="pointer-events-none absolute right-1.5 size-3.5 text-ink-3"
      />
    </span>
  )
}

function FilterToggle({
  label,
  hint,
  active,
  badge,
  onClick,
}: {
  label: string
  hint: string
  active: boolean
  badge?: number
  onClick(): void
}) {
  return (
    <button
      aria-label={hint}
      aria-pressed={active}
      className={cn(
        'inline-flex h-8 items-center gap-1.5 rounded-md border px-2.5 text-[12px]',
        active
          ? 'border-cyanotype-blue/40 bg-cyanotype-blue/10 text-cyanotype-blue'
          : 'border-ledger-border text-ink-2 hover:bg-card-tint',
      )}
      onClick={onClick}
      type="button"
    >
      {label}
      {badge !== undefined ? (
        <span className="rounded-full bg-cyanotype-blue/10 px-1.5 text-[10px]">
          {badge}
        </span>
      ) : null}
    </button>
  )
}

export function RequestFilters({
  state,
  onChange,
  onReset,
  typeOptions,
  tabOptions,
  collectorCount,
}: {
  state: NetworkSearchState
  onChange(patch: Partial<NetworkSearchState>): void
  onReset(): void
  typeOptions: string[]
  tabOptions: string[]
  collectorCount: number
}) {
  const [query, setQuery] = useState(state.query)
  const focused = useRef(false)

  // Back/forward changes the URL; mirror it while the input is not focused.
  useEffect(() => {
    if (!focused.current) setQuery(state.query)
  }, [state.query])

  // The parent re-renders on every streamed record, so the debounce must not
  // depend on `onChange`'s identity or the timer would reset forever.
  const onChangeRef = useRef(onChange)
  useEffect(() => {
    onChangeRef.current = onChange
  })
  useEffect(() => {
    if (query === state.query) return
    const timer = setTimeout(() => onChangeRef.current({ query }), 150)
    return () => clearTimeout(timer)
  }, [query, state.query])

  return (
    <div className="mb-3 flex shrink-0 flex-col gap-2 rounded-9 border border-ledger-border bg-card px-3 py-2.5">
      <div className="flex flex-wrap items-center gap-2">
        <div className="relative min-w-[220px] max-w-md flex-1">
          <Search
            aria-hidden
            className="absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2 text-ink-3"
          />
          <input
            aria-label={copy.searchLabel}
            className="h-9 w-full rounded-md border border-ledger-border bg-background pr-8 pl-8 text-[12.5px]"
            onBlur={() => {
              focused.current = false
            }}
            onChange={(event) => setQuery(event.target.value)}
            onFocus={() => {
              focused.current = true
            }}
            placeholder={copy.searchPlaceholder}
            type="search"
            value={query}
          />
          {query ? (
            <button
              aria-label={copy.clearSearch}
              className="absolute top-1/2 right-2 -translate-y-1/2 text-ink-3 hover:text-ink"
              onClick={() => {
                setQuery('')
                onChange({ query: '' })
              }}
              type="button"
            >
              <X className="size-3.5" />
            </button>
          ) : null}
        </div>

        <FilterSelect
          label={copy.methodLabel}
          onChange={(method) => onChange({ method })}
          options={[
            { value: 'all', label: copy.allMethods },
            ...METHODS.map((method) => ({ value: method, label: method })),
          ]}
          value={state.method}
        />
        <FilterSelect
          label={copy.statusLabel}
          onChange={(status) => onChange({ status: status as RecordFacet })}
          options={STATUS_FACETS.map((facet) => ({
            value: facet,
            label: copy.status[facet],
          }))}
          value={state.status}
        />
        <FilterSelect
          label={copy.typeLabel}
          onChange={(resourceType) => onChange({ resourceType })}
          options={[
            { value: 'all', label: copy.allTypes },
            ...typeOptions.map((type) => ({ value: type, label: type })),
          ]}
          value={state.resourceType}
        />
        <FilterSelect
          label={copy.tabLabel}
          onChange={(tab) => onChange({ tab })}
          options={[
            { value: 'all', label: copy.allTabs },
            ...tabOptions.map((tab) => ({ value: tab, label: tab })),
          ]}
          value={state.tab}
        />
      </div>

      <div className="flex flex-wrap items-center gap-2">
        <FilterToggle
          active={state.mock}
          hint={copy.mockOnlyHint}
          label={copy.mockOnly}
          onClick={() => onChange({ mock: !state.mock })}
        />
        <FilterToggle
          active={state.body}
          hint={copy.hasBodyHint}
          label={copy.hasBody}
          onClick={() => onChange({ body: !state.body })}
        />
        <FilterToggle
          active={state.hideAgent}
          badge={collectorCount}
          hint={copy.hideAgentHint}
          label={state.hideAgent ? copy.agentHidden : copy.agentShown}
          onClick={() => onChange({ hideAgent: !state.hideAgent })}
        />
        <button
          className="ml-auto text-[12px] text-ink-3 hover:text-ink"
          onClick={onReset}
          type="button"
        >
          {copy.reset}
        </button>
      </div>
    </div>
  )
}
