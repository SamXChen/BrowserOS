import type { NetworkBodyPage } from '@browseros/claw-api'
import { useEffect, useMemo, useState } from 'react'
import { toast } from 'sonner'
import { HeadersPane, TextBodyPane } from '@/components/network/BodyPanes'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import {
  useNetworkRequestBody,
  useNetworkResponseBody,
} from '@/modules/api/network.hooks'
import { networkCopy } from '@/screens/network/network.copy'
import { shouldLoadResponseBody } from '@/screens/network/network.data'
import {
  type DerivedRecord,
  decodeBase64Pages,
  formatBytes,
  formatDuration,
  formatTime,
  prettify,
} from '@/screens/network/network.helpers'

const copy = networkCopy.detail
const PAGE_LIMIT = 65_536

function fileNameFor(record: DerivedRecord): string {
  const path = record.derived?.path ?? record.url
  const last = path.split('?')[0]?.split('/').filter(Boolean).pop()
  return last || 'response.bin'
}

export function RequestDetail({
  record,
  ruleName,
  onOpenRule,
}: {
  record: DerivedRecord | null
  ruleName?: string
  onOpenRule?(ruleId: string): void
}) {
  const [tab, setTab] = useState('general')
  const [pages, setPages] = useState<NetworkBodyPage[]>([])
  const [offset, setOffset] = useState(0)
  const recordId = record?.recordId ?? ''

  useEffect(() => {
    setTab('general')
    setPages([])
    setOffset(0)
  }, [])

  const responseQuery = useNetworkResponseBody({
    variables: { recordId, offset, limit: PAGE_LIMIT },
    enabled:
      Boolean(recordId) && shouldLoadResponseBody(tab, record?.bodyState),
  })

  useEffect(() => {
    const page = responseQuery.data
    if (!page) return
    setPages((current) =>
      current.some((entry) => entry.offset === page.offset)
        ? current
        : [...current, page].sort((left, right) => left.offset - right.offset),
    )
  }, [responseQuery.data])

  const payloadQuery = useNetworkRequestBody({
    variables: { recordId, offset: 0, limit: 262_144 },
    enabled: Boolean(recordId) && tab === 'payload',
  })

  const binary = pages.some((page) => page.encoding === 'base64')
  const text = useMemo(
    () => (binary ? '' : pages.map((page) => page.data).join('')),
    [binary, pages],
  )
  const lastPage = pages.at(-1)

  if (!record) {
    return (
      <div className="flex h-full items-center justify-center px-6 text-[12px] text-ink-3">
        {copy.title}
      </div>
    )
  }

  const download = (): void => {
    const bytes = decodeBase64Pages(pages)
    const url = URL.createObjectURL(new Blob([bytes as BlobPart]))
    const anchor = document.createElement('a')
    anchor.href = url
    anchor.download = fileNameFor(record)
    anchor.click()
    setTimeout(() => URL.revokeObjectURL(url), 1000)
  }

  const copyBody = async (): Promise<void> => {
    try {
      await navigator.clipboard.writeText(text)
      toast(copy.copied)
    } catch (cause) {
      toast.error(
        copy.copyFailed(cause instanceof Error ? cause.message : String(cause)),
      )
    }
  }

  const duration =
    record.finishedAtMs !== undefined
      ? record.finishedAtMs - record.startedAtMs
      : undefined

  return (
    <div className="flex h-full min-h-0 flex-col">
      <header className="flex shrink-0 flex-col gap-1 border-ledger-border border-b px-3 py-2.5">
        <span className="text-[12px] text-ink-2">{copy.title}</span>
        <span className="truncate font-mono text-[11px] text-ink-3">
          {record.url}
        </span>
        {record.mock ? (
          <span className="flex items-center gap-1.5 text-[11px] text-ink-2">
            {record.mock.outcome === 'processed'
              ? copy.processorBy
              : copy.mockBy}
            {ruleName && onOpenRule ? (
              <button
                className="text-cyanotype-blue hover:underline"
                onClick={() => onOpenRule(record.mock?.ruleId ?? '')}
                type="button"
              >
                {ruleName}
              </button>
            ) : null}
          </span>
        ) : null}
      </header>

      <Tabs
        className="flex min-h-0 flex-1 flex-col gap-0"
        onValueChange={setTab}
        value={tab}
      >
        <TabsList className="h-9 shrink-0 justify-start rounded-none border-ledger-border border-b bg-transparent px-2">
          {(
            [
              ['general', copy.tabs.general],
              ['timing', copy.tabs.timing],
              ['headers', copy.tabs.headers],
              ['payload', copy.tabs.payload],
              ['response', copy.tabs.response],
            ] as const
          ).map(([value, label]) => (
            <TabsTrigger
              className="h-7 rounded-md border-0 px-2.5 text-[11.5px] text-ink-3 data-active:bg-card-tint data-active:text-ink"
              key={value}
              value={value}
            >
              {label}
            </TabsTrigger>
          ))}
        </TabsList>

        <div className="min-h-0 flex-1 overflow-auto px-3 py-2">
          <TabsContent value="general">
            <dl className="grid grid-cols-[120px_minmax(0,1fr)] gap-x-4 gap-y-1 text-[11.5px]">
              <dt className="text-ink-3">{copy.general.request}</dt>
              <dd className="break-all font-mono text-ink-2">
                {record.method} {record.url}
              </dd>
              <dt className="text-ink-3">{copy.general.status}</dt>
              <dd className="text-ink-2">
                {record.status ?? record.failure ?? '—'}
              </dd>
              <dt className="text-ink-3">{copy.general.mimeType}</dt>
              <dd className="text-ink-2">{record.mimeType ?? '—'}</dd>
              <dt className="text-ink-3">{copy.general.size}</dt>
              <dd className="text-ink-2">
                {record.encodedDataLength !== undefined
                  ? formatBytes(record.encodedDataLength)
                  : '—'}
              </dd>
              <dt className="text-ink-3">{copy.general.started}</dt>
              <dd className="text-ink-2">{formatTime(record.startedAtMs)}</dd>
              <dt className="text-ink-3">{copy.general.duration}</dt>
              <dd className="text-ink-2">
                {duration !== undefined ? formatDuration(duration) : '—'}
              </dd>
              <dt className="text-ink-3">{copy.general.cache}</dt>
              <dd className="text-ink-2">
                {record.fromCache ? copy.general.yes : copy.general.no}
              </dd>
              <dt className="text-ink-3">{copy.general.serviceWorker}</dt>
              <dd className="text-ink-2">
                {record.fromServiceWorker ? copy.general.yes : copy.general.no}
              </dd>
              <dt className="text-ink-3">{copy.general.source}</dt>
              <dd className="text-ink-2">{record.tabTitle ?? '—'}</dd>
            </dl>
          </TabsContent>

          <TabsContent value="timing">
            <dl className="grid grid-cols-[120px_minmax(0,1fr)] gap-x-4 gap-y-1 text-[11.5px]">
              <dt className="text-ink-3">{copy.timing.total}</dt>
              <dd className="text-ink-2">
                {duration !== undefined ? formatDuration(duration) : '—'}
              </dd>
              <dt className="text-ink-3">{copy.timing.finished}</dt>
              <dd className="text-ink-2">
                {record.finishedAtMs ? formatTime(record.finishedAtMs) : '—'}
              </dd>
            </dl>
          </TabsContent>

          <TabsContent value="headers">
            <HeadersPane
              empty={copy.responseEmpty}
              headers={record.responseHeaders}
            />
          </TabsContent>

          <TabsContent value="payload">
            <TextBodyPane
              binary={payloadQuery.data?.encoding === 'base64'}
              empty={copy.payloadEmpty}
              error={payloadQuery.isError ? copy.bodyLoadFailed : undefined}
              hasMore={false}
              loading={payloadQuery.isPending}
              onDownload={() => {}}
              onLoadMore={() => {}}
              text={prettify(payloadQuery.data?.data ?? '')}
            />
          </TabsContent>

          <TabsContent value="response">
            {record.mock?.outcome === 'processed' ? (
              <p className="mb-1.5 text-[11px] text-ink-3">
                {copy.processorHint}
              </p>
            ) : null}
            <TextBodyPane
              binary={binary}
              empty={copy.responseEmpty}
              error={
                responseQuery.isError && pages.length === 0
                  ? copy.bodyLoadFailed
                  : undefined
              }
              hasMore={Boolean(lastPage?.truncated)}
              loading={responseQuery.isFetching}
              onCopy={() => void copyBody()}
              onDownload={download}
              onLoadMore={() => setOffset((lastPage?.offset ?? 0) + PAGE_LIMIT)}
              text={prettify(text)}
            />
          </TabsContent>
        </div>
      </Tabs>
    </div>
  )
}
