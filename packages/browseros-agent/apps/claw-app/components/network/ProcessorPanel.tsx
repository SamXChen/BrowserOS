import type { NetworkProcessorTestResult } from '@browseros/claw-api'
import { useState } from 'react'
import { CodeBlock } from '@/components/ai-elements/code-block'
import { Button } from '@/components/ui/button'
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from '@/components/ui/tooltip'
import { cn } from '@/lib/utils'
import { useTestNetworkProcessor } from '@/modules/api/network.hooks'
import { networkCopy } from '@/screens/network/network.copy'
import { prettify } from '@/screens/network/network.helpers'

const copy = networkCopy.rules.detail

const OUTCOME_TONE: Record<string, string> = {
  processed: 'border-cyanotype-blue/40 text-cyanotype-blue',
  passthrough: 'border-ledger-border text-ink-3',
  error: 'border-red-500/40 text-red-600 dark:text-red-400',
  timeout: 'border-red-500/40 text-red-600 dark:text-red-400',
}

export function ProcessorPanel({
  ruleId,
  script,
  timeoutMs,
  sampleAvailable,
}: {
  ruleId: string
  script: string
  timeoutMs: number
  sampleAvailable: boolean
}) {
  const test = useTestNetworkProcessor()
  const [result, setResult] = useState<NetworkProcessorTestResult | null>(null)
  const [error, setError] = useState<string | null>(null)

  const run = async (): Promise<void> => {
    setError(null)
    try {
      setResult(await test.mutateAsync({ id: ruleId }))
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    }
  }

  const button = (
    <Button
      disabled={!sampleAvailable || test.isPending}
      onClick={() => void run()}
      size="xs"
      variant="outline"
    >
      {test.isPending ? copy.processorTesting : copy.processorTest}
    </Button>
  )

  return (
    <div className="flex flex-col gap-2">
      <div className="max-h-80 overflow-auto rounded-md">
        <CodeBlock code={script} language="javascript" />
      </div>
      <div className="flex flex-wrap items-center gap-2">
        <span className="text-[11px] text-ink-3">
          {copy.processorTimeout(timeoutMs)}
        </span>
        {sampleAvailable ? (
          button
        ) : (
          <Tooltip>
            <TooltipTrigger render={<span className="inline-flex" />}>
              {button}
            </TooltipTrigger>
            <TooltipContent>{copy.processorNoSample}</TooltipContent>
          </Tooltip>
        )}
      </div>

      {error ? (
        <p className="text-[11px] text-red-600 dark:text-red-400">
          {copy.processorError(error)}
        </p>
      ) : null}

      {result ? (
        <div className="flex flex-col gap-1.5 rounded border border-ledger-border px-2 py-1.5">
          <div className="flex flex-wrap items-center gap-2 text-[11.5px]">
            <span
              className={cn(
                'rounded border px-1.5 py-0.5 font-medium text-[10.5px]',
                OUTCOME_TONE[result.outcome] ?? OUTCOME_TONE.passthrough,
              )}
            >
              {copy.processorOutcome[result.outcome] ?? result.outcome}
            </span>
            {result.status !== undefined ? (
              <span className="font-mono text-ink-2">
                {copy.processorMeta(result.status, result.elapsedMs ?? 0)}
              </span>
            ) : null}
          </div>

          {result.headers?.length ? (
            <div className="flex flex-col gap-0.5">
              <span className="font-semibold text-[11px] text-ink-2">
                {copy.processorHeaders}
              </span>
              <div className="flex max-h-40 flex-col overflow-auto font-mono text-[11px] text-ink-2">
                {result.headers.map((header) => (
                  <span key={`${header.name}:${header.value.slice(0, 24)}`}>
                    {header.name}: {header.value}
                  </span>
                ))}
              </div>
            </div>
          ) : null}

          {result.body?.preview ? (
            <div className="flex flex-col gap-0.5">
              <span className="font-semibold text-[11px] text-ink-2">
                {copy.processorBody}
              </span>
              <pre className="max-h-60 overflow-auto rounded bg-card-tint px-2 py-1.5 font-mono text-[11px]">
                {prettify(result.body.preview)}
              </pre>
            </div>
          ) : null}

          {result.logs?.length ? (
            <div className="flex flex-col gap-0.5">
              <span className="font-semibold text-[11px] text-ink-2">
                {copy.processorLogs}
              </span>
              <pre className="max-h-40 overflow-auto font-mono text-[11px] text-ink-3">
                {result.logs.join('\n')}
              </pre>
            </div>
          ) : null}

          {result.error ? (
            <p className="text-[11px] text-red-600 dark:text-red-400">
              {copy.processorError(result.error)}
            </p>
          ) : null}
        </div>
      ) : null}
    </div>
  )
}
