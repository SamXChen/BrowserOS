import type { NetworkRule, NetworkRuleTestResult } from '@browseros/claw-api'
import { useState } from 'react'
import { ProcessorPanel } from '@/components/network/ProcessorPanel'
import { Alert, AlertDescription } from '@/components/ui/alert'
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '@/components/ui/alert-dialog'
import { Button } from '@/components/ui/button'
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from '@/components/ui/collapsible'
import {
  useNetworkRule,
  useNetworkRuleRevisions,
} from '@/modules/api/network.hooks'
import { toast } from '@/modules/toast/toast'
import { networkCopy } from '@/screens/network/network.copy'
import {
  diffRevisions,
  formatTime,
  prettify,
} from '@/screens/network/network.helpers'

const copy = networkCopy.rules.detail

export function RuleDetail({
  rule,
  onRevert,
  onTest,
}: {
  rule: NetworkRule
  onRevert(revision: number): Promise<unknown>
  onTest(): Promise<NetworkRuleTestResult>
}) {
  const detail = useNetworkRule({ variables: { id: rule.id } })
  const revisions = useNetworkRuleRevisions({ variables: { id: rule.id } })
  const [revertTo, setRevertTo] = useState<number | null>(null)
  const [testing, setTesting] = useState(false)
  const [result, setResult] = useState<NetworkRuleTestResult | null>(null)
  const [error, setError] = useState<string | null>(null)

  const current = detail.data?.revision
  const matcher = current?.matcher
  const action = current?.action
  const hits = detail.data?.hits ?? []
  const history = (revisions.data?.revisions ?? [])
    .slice()
    .sort((left, right) => right.revision - left.revision)

  const runTest = async (): Promise<void> => {
    setError(null)
    setTesting(true)
    try {
      setResult(await onTest())
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setTesting(false)
    }
  }

  return (
    <div className="mt-1 flex flex-col gap-3 border-ledger-border border-t pt-2.5 text-[11.5px]">
      {error ? (
        <Alert variant="destructive">
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      ) : null}

      <section className="flex flex-col gap-1.5">
        <h4 className="font-semibold text-[11.5px] text-ink-2">
          {copy.matcher}
        </h4>
        <dl className="grid grid-cols-[110px_minmax(0,1fr)] gap-x-3 gap-y-1">
          <dt className="text-ink-3">URL</dt>
          <dd className="break-all font-mono text-ink-2">
            {matcher
              ? `${matcher.method} ${matcher.origin ?? ''}${matcher.pathname ?? ''}`
              : '—'}
          </dd>
          <dt className="text-ink-3">Query</dt>
          <dd className="text-ink-2">
            {matcher?.queryPolicy?.mode === 'strict'
              ? copy.strict
              : copy.lenient}
            {matcher?.queryPolicy?.ignoreKeys?.length
              ? ` · ${copy.ignoreKeys}: ${matcher.queryPolicy.ignoreKeys.join(', ')}`
              : ''}
          </dd>
          {matcher?.sampleQuery?.length ? (
            <>
              <dt className="text-ink-3">{copy.sampleQuery}</dt>
              <dd className="font-mono text-ink-2">
                {matcher.sampleQuery
                  .map(([key, value]) => `${key}=${value}`)
                  .join(' · ')}
              </dd>
            </>
          ) : null}
          {matcher?.bodyFields?.length ? (
            <>
              <dt className="text-ink-3">{copy.bodyFields}</dt>
              <dd className="text-ink-2">
                {matcher.bodyFields
                  .map(
                    (field) =>
                      `${field.pointer} = ${JSON.stringify(field.equals)}`,
                  )
                  .join(' · ')}
              </dd>
            </>
          ) : null}
          {matcher?.resourceType ? (
            <>
              <dt className="text-ink-3">{copy.resourceType}</dt>
              <dd className="text-ink-2">{matcher.resourceType}</dd>
            </>
          ) : null}
        </dl>
      </section>

      <section className="flex flex-col gap-1.5">
        <h4 className="font-semibold text-[11.5px] text-ink-2">
          {copy.response}
        </h4>
        {action?.kind === 'processor' ? (
          <ProcessorPanel
            ruleId={rule.id}
            sampleAvailable={detail.data?.sample?.available ?? false}
            script={action.script ?? ''}
            timeoutMs={action.timeoutMs ?? 200}
          />
        ) : (
          <dl className="grid grid-cols-[110px_minmax(0,1fr)] gap-x-3 gap-y-1">
            <dt className="text-ink-3">{copy.responseStatus}</dt>
            <dd className="text-ink-2">{action?.status ?? 200}</dd>
            <dt className="text-ink-3">{copy.responseDelay}</dt>
            <dd className="text-ink-2">{action?.delayMs ?? 0}ms</dd>
            {action?.headers && Object.keys(action.headers).length > 0 ? (
              <>
                <dt className="text-ink-3">{copy.responseHeaders}</dt>
                <dd className="flex flex-col gap-0.5 font-mono text-ink-2">
                  {Object.entries(action.headers).map(([name, value]) => (
                    <span key={name}>
                      {name}:{' '}
                      {value === null
                        ? copy.headerDelete
                        : Array.isArray(value)
                          ? value.join(' | ')
                          : String(value)}
                    </span>
                  ))}
                </dd>
              </>
            ) : null}
          </dl>
        )}
      </section>

      <section className="flex flex-col gap-1">
        <h4 className="font-semibold text-[11.5px] text-ink-2">
          {copy.source}
        </h4>
        <span className="font-mono text-ink-3">
          {copy.sourceHandoff}: {current?.source.handoffId ?? '—'}
        </span>
        <span className="font-mono text-ink-3">
          {copy.sourceRecord}: {current?.source.recordId ?? '—'}
        </span>
        <span className="text-ink-3">
          {detail.data?.sample?.available
            ? copy.sampleAvailable
            : copy.sampleMissing}
        </span>
      </section>

      {action?.kind === 'processor' ? (
        <Collapsible>
          <CollapsibleTrigger className="text-left font-semibold text-[11.5px] text-ink-2">
            {copy.limits}
          </CollapsibleTrigger>
          <CollapsibleContent>
            <ul className="mt-1 flex list-disc flex-col gap-0.5 pl-4 text-[11px] text-ink-3">
              {copy.limitsList.map((line) => (
                <li key={line}>{line}</li>
              ))}
            </ul>
          </CollapsibleContent>
        </Collapsible>
      ) : null}

      <section className="flex flex-col gap-1">
        <h4 className="font-semibold text-[11.5px] text-ink-2">
          {copy.recentHits}
        </h4>
        {hits.length === 0 ? (
          <span className="text-ink-3">{copy.noHits}</span>
        ) : (
          hits.slice(0, 10).map((hit) => (
            <span
              className="truncate font-mono text-[11px] text-ink-3"
              key={hit.id}
            >
              {formatTime(hit.at)} · {hit.outcome} · {hit.url}
            </span>
          ))
        )}
      </section>

      <section className="flex flex-col gap-1.5">
        <div className="flex items-center justify-between">
          <h4 className="font-semibold text-[11.5px] text-ink-2">
            {copy.revisions}
          </h4>
          <Button
            disabled={testing}
            onClick={() => void runTest()}
            size="xs"
            variant="outline"
          >
            {testing ? copy.testing : copy.test}
          </Button>
        </div>
        {result ? (
          <div className="flex flex-col gap-0.5">
            <span className="text-ink-2">
              {copy.testResult(result.matched, result.considered)}
            </span>
            {result.samples.slice(0, 5).map((sample) => (
              <span
                className="truncate font-mono text-[11px] text-ink-3"
                key={sample.recordId}
              >
                {sample.method} {sample.url} · {sample.status ?? '—'}
              </span>
            ))}
          </div>
        ) : null}
        {history.map((revision, index) => {
          const previous = history[index + 1]
          const changes = previous ? diffRevisions(previous, revision) : []
          return (
            <div
              className="flex flex-wrap items-center gap-2 rounded border border-ledger-border px-2 py-1"
              key={revision.revision}
            >
              <span className="font-mono text-ink-2">v{revision.revision}</span>
              <span className="text-ink-3">
                {revision.createdAt ? formatTime(revision.createdAt) : '—'}
              </span>
              <span className="text-ink-3">
                {changes.length > 0 ? copy.changed(changes) : copy.noChanges}
              </span>
              <Button
                className="ml-auto"
                disabled={revision.revision === rule.currentRevision}
                onClick={() => setRevertTo(revision.revision)}
                size="xs"
                variant="ghost"
              >
                {copy.revert}
              </Button>
            </div>
          )
        })}
        {action?.body?.preview ? (
          <pre className="max-h-40 overflow-auto rounded bg-card-tint px-2 py-1.5 font-mono text-[11px]">
            {prettify(action.body.preview)}
          </pre>
        ) : null}
      </section>

      <AlertDialog
        onOpenChange={(open) => {
          if (!open) setRevertTo(null)
        }}
        open={revertTo !== null}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              {copy.revertTitle(revertTo ?? 0)}
            </AlertDialogTitle>
            <AlertDialogDescription>
              {copy.revertBody(revertTo ?? 0, rule.currentRevision + 1)}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{networkCopy.capture.cancel}</AlertDialogCancel>
            <AlertDialogAction
              onClick={() => {
                const revision = revertTo
                setRevertTo(null)
                if (revision === null) return
                void onRevert(revision).then(
                  () => toast(copy.reverted(rule.currentRevision + 1)),
                  (cause: unknown) =>
                    setError(
                      cause instanceof Error ? cause.message : String(cause),
                    ),
                )
              }}
            >
              {copy.revert}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  )
}
