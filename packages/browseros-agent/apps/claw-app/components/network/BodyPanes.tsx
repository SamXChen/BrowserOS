import type { NetworkHeaderPair } from '@browseros/claw-api'
import { Copy, Download } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { networkCopy } from '@/screens/network/network.copy'

const copy = networkCopy.detail

export function HeadersPane({
  headers,
  empty,
}: {
  headers?: NetworkHeaderPair[]
  empty: string
}) {
  if (!headers || headers.length === 0) {
    return <p className="px-1 py-3 text-[12px] text-ink-3">{empty}</p>
  }
  return (
    <dl className="grid grid-cols-[minmax(140px,220px)_minmax(0,1fr)] gap-x-4 gap-y-1 px-1 py-2 font-mono text-[11.5px]">
      {headers.map((header) => (
        <div
          className="contents"
          key={`${header.name}:${header.value.slice(0, 24)}`}
        >
          <dt className="truncate text-ink-3">{header.name}</dt>
          <dd className="break-all text-ink-2">{header.value}</dd>
        </div>
      ))}
    </dl>
  )
}

export function TextBodyPane({
  text,
  empty,
  error,
  loading,
  hasMore,
  binary,
  onLoadMore,
  onDownload,
  onCopy,
}: {
  text: string
  empty: string
  error?: string | null
  loading: boolean
  hasMore: boolean
  binary: boolean
  onLoadMore(): void
  onDownload(): void
  onCopy?(): void
}) {
  return (
    <div className="flex flex-col gap-2">
      <div className="flex items-center justify-end gap-2">
        {onCopy ? (
          <Button
            aria-label={copy.copyResponseBody}
            disabled={binary}
            onClick={onCopy}
            size="xs"
            variant="outline"
          >
            <Copy className="size-3" />
            {copy.copyBody}
          </Button>
        ) : null}
        <Button onClick={onDownload} size="xs" variant="outline">
          <Download className="size-3" />
          {copy.download}
        </Button>
      </div>
      {error ? (
        <p className="px-1 py-3 text-[12px] text-red-600">{error}</p>
      ) : text ? (
        <pre className="max-h-[420px] overflow-auto whitespace-pre-wrap break-all rounded-md bg-card-tint px-3 py-2 font-mono text-[11.5px]">
          {text}
        </pre>
      ) : loading ? (
        <p className="px-1 py-3 text-[12px] text-ink-3">{copy.loading}</p>
      ) : (
        <p className="px-1 py-3 text-[12px] text-ink-3">{empty}</p>
      )}
      {hasMore ? (
        <Button
          className="self-start"
          disabled={loading}
          onClick={onLoadMore}
          size="xs"
          variant="ghost"
        >
          {copy.loadMore}
        </Button>
      ) : null}
    </div>
  )
}
