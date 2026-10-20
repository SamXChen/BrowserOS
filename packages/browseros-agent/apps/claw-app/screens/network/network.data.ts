import { useCallback, useEffect, useRef, useState } from 'react'
import { toast } from 'sonner'
import {
  type BodyPage,
  type NetworkRecord,
  type NetworkRule,
  type NetworkStatus,
  networkApi,
} from '@/modules/api/network.api'

const RECORD_RENDER_LIMIT = 500

function upsertRecord(
  records: NetworkRecord[],
  next: NetworkRecord,
): NetworkRecord[] {
  const index = records.findIndex((record) => record.recordId === next.recordId)
  if (index === -1) {
    return [next, ...records].slice(0, RECORD_RENDER_LIMIT)
  }
  const copy = records.slice()
  const existing = copy[index]
  copy[index] = {
    ...existing,
    ...next,
    // A later update may omit a body state the body worker already set.
    bodyState: next.bodyState ?? existing.bodyState,
  }
  return copy
}

async function writeClipboard(text: string): Promise<void> {
  if (navigator.clipboard?.writeText) {
    await navigator.clipboard.writeText(text)
    return
  }
  const area = document.createElement('textarea')
  area.value = text
  area.style.position = 'fixed'
  area.style.opacity = '0'
  document.body.appendChild(area)
  area.select()
  document.execCommand('copy')
  document.body.removeChild(area)
}

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

export function useNetworkScreen() {
  const [status, setStatus] = useState<NetworkStatus | null>(null)
  const [records, setRecords] = useState<NetworkRecord[]>([])
  const [rules, setRules] = useState<NetworkRule[]>([])
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const [detailBody, setDetailBody] = useState<BodyPage | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const mounted = useRef(true)

  const refresh = useCallback(async () => {
    try {
      const [nextStatus, nextRecords, nextRules] = await Promise.all([
        networkApi.state(),
        networkApi.records(RECORD_RENDER_LIMIT),
        networkApi.rules(),
      ])
      if (!mounted.current) return
      setStatus(nextStatus)
      setRecords(nextRecords.records.slice().reverse())
      setRules(nextRules.rules)
      setError(null)
    } catch (cause) {
      if (mounted.current) setError(errorText(cause))
    }
  }, [])

  useEffect(() => {
    mounted.current = true
    void refresh()
    let source: EventSource | null = null
    let closed = false

    void networkApi.eventUrl().then((url) => {
      if (closed) return
      source = new EventSource(url)
      source.addEventListener('snapshot', (event) => {
        const payload = JSON.parse((event as MessageEvent).data)
        setStatus({
          browser: payload.browser ?? { connected: false },
          capture: payload.capture,
          rules: payload.rules ?? { active: 0, total: 0 },
        })
      })
      source.addEventListener('record', (event) => {
        const payload = JSON.parse((event as MessageEvent).data)
        if (!payload.record) return
        setRecords((current) => upsertRecord(current, payload.record))
      })
      source.addEventListener('record_done', (event) => {
        const payload = JSON.parse((event as MessageEvent).data)
        setRecords((current) =>
          current.map((record) =>
            record.recordId === payload.recordId
              ? {
                  ...record,
                  bodyState: payload.bodyState,
                  bodyReason: payload.bodyReason,
                }
              : record,
          ),
        )
      })
      source.addEventListener('rule', (event) => {
        const payload = JSON.parse((event as MessageEvent).data) as NetworkRule
        setRules((current) => {
          const index = current.findIndex((rule) => rule.id === payload.id)
          if (index === -1) return [payload, ...current]
          const copy = current.slice()
          copy[index] = payload
          return copy
        })
        void refresh()
      })
      source.addEventListener('hit', (event) => {
        const payload = JSON.parse((event as MessageEvent).data)
        setRules((current) =>
          current.map((rule) =>
            rule.id === payload.ruleId
              ? {
                  ...rule,
                  lastHit: {
                    at: payload.at,
                    outcome: payload.outcome,
                    url: payload.url,
                    method: payload.method,
                    tabTitle: payload.tabTitle,
                  },
                }
              : rule,
          ),
        )
      })
      source.addEventListener('capture', (event) => {
        const payload = JSON.parse((event as MessageEvent).data)
        setStatus((current) =>
          current ? { ...current, capture: payload } : current,
        )
      })
      source.addEventListener('evicted', (event) => {
        const payload = JSON.parse((event as MessageEvent).data)
        if (!payload.recordId) return
        setRecords((current) =>
          current.filter((record) => record.recordId !== payload.recordId),
        )
        setSelectedId((current) => {
          if (current !== payload.recordId) return current
          toast('该记录已被淘汰')
          return null
        })
      })
      source.addEventListener('gap', (event) => {
        const payload = JSON.parse((event as MessageEvent).data)
        if (payload.reset) {
          toast('事件流已重置，正在重新同步')
          void refresh()
          return
        }
        if (payload.lost) {
          setStatus((current) =>
            current
              ? {
                  ...current,
                  capture: {
                    ...current.capture,
                    gapEvents: current.capture.gapEvents + payload.lost,
                  },
                }
              : current,
          )
        }
      })
      source.addEventListener('deleted', (event) => {
        const payload = JSON.parse((event as MessageEvent).data)
        if (payload.kind === 'record') {
          if (payload.all) {
            setRecords([])
            setSelectedId(null)
          } else if (payload.id) {
            setRecords((current) =>
              current.filter((record) => record.recordId !== payload.id),
            )
            setSelectedId((current) =>
              current === payload.id ? null : current,
            )
          }
        }
        if (payload.kind === 'rule' && payload.id) {
          setRules((current) =>
            current.filter((rule) => rule.id !== payload.id),
          )
        }
      })
      source.onerror = () => {
        // EventSource reconnects on its own; keep the last snapshot on screen.
      }
    })

    const poll = window.setInterval(() => {
      void refresh()
    }, 10_000)

    return () => {
      closed = true
      mounted.current = false
      window.clearInterval(poll)
      source?.close()
    }
  }, [refresh])

  const selected =
    records.find((record) => record.recordId === selectedId) ?? null

  useEffect(() => {
    setDetailBody(null)
    if (!selectedId) return
    let current = true
    void networkApi
      .responseBody(selectedId)
      .then((page) => {
        if (current) setDetailBody(page)
      })
      .catch(() => {
        if (current) setDetailBody(null)
      })
    return () => {
      current = false
    }
  }, [selectedId])

  const run = useCallback(async (action: () => Promise<void>) => {
    setBusy(true)
    try {
      await action()
    } catch (cause) {
      toast.error(errorText(cause))
    } finally {
      if (mounted.current) setBusy(false)
    }
  }, [])

  const startCapture = useCallback(
    () =>
      run(async () => {
        const capture = await networkApi.startCapture()
        setStatus((current) => (current ? { ...current, capture } : current))
        toast.success('已开始采集')
      }),
    [run],
  )

  const stopCapture = useCallback(
    () =>
      run(async () => {
        const capture = await networkApi.stopCapture()
        setStatus((current) => (current ? { ...current, capture } : current))
        setRecords([])
        setSelectedId(null)
        toast.success('已停止采集')
      }),
    [run],
  )

  const clearRecords = useCallback(
    () =>
      run(async () => {
        await networkApi.deleteRecords()
        setRecords([])
        setSelectedId(null)
        toast.success('已清空列表')
      }),
    [run],
  )

  const copyReference = useCallback(
    (record: NetworkRecord) =>
      run(async () => {
        const handoff = await networkApi.createHandoff(record.recordId)
        await writeClipboard(handoff.clipboardText)
        toast.success('已复制引用')
      }),
    [run],
  )

  const deleteRecord = useCallback(
    (record: NetworkRecord) =>
      run(async () => {
        const result = await networkApi.deleteRecord(record.recordId)
        setRecords((current) =>
          current.filter((item) => item.recordId !== record.recordId),
        )
        setSelectedId((current) =>
          current === record.recordId ? null : current,
        )
        toast('已删除 1 条', {
          action: {
            label: '撤销',
            onClick: () => {
              void networkApi
                .restoreRecord(result.undoToken)
                .then(refresh)
                .catch((cause) => toast.error(errorText(cause)))
            },
          },
        })
      }),
    [refresh, run],
  )

  const toggleRule = useCallback(
    (rule: NetworkRule, enabled: boolean) =>
      run(async () => {
        setRules((current) =>
          current.map((item) =>
            item.id === rule.id ? { ...item, enabled } : item,
          ),
        )
        try {
          const result = await networkApi.setRuleEnabled(rule.id, enabled)
          setRules((current) =>
            current.map((item) =>
              item.id === rule.id
                ? { ...item, enabled: result.enabled, state: result.state }
                : item,
            ),
          )
        } catch (cause) {
          setRules((current) =>
            current.map((item) =>
              item.id === rule.id ? { ...item, enabled: !enabled } : item,
            ),
          )
          throw cause
        }
      }),
    [run],
  )

  const deleteRule = useCallback(
    (rule: NetworkRule) =>
      run(async () => {
        await networkApi.deleteRule(rule.id)
        setRules((current) => current.filter((item) => item.id !== rule.id))
        toast('已删除规则', {
          action: {
            label: '撤销',
            onClick: () => {
              void networkApi
                .restoreRule(rule.id)
                .then(refresh)
                .catch((cause) => toast.error(errorText(cause)))
            },
          },
        })
      }),
    [refresh, run],
  )

  const fixNearMiss = useCallback(
    (rule: NetworkRule) =>
      run(async () => {
        const result = await networkApi.nearMisses(rule.id)
        const keys = Array.from(
          new Set(
            result.nearMisses.flatMap(
              (miss) => miss.suggestion?.ignoreKeys ?? [],
            ),
          ),
        )
        if (keys.length === 0) {
          toast.error('没有发现可自动忽略的 query 差异')
          return
        }
        const fixed = await networkApi.ignoreKeys(rule.id, keys)
        await refresh()
        toast.success(
          `已忽略 ${keys.join(', ')}，规则重建为 v${fixed.revision}`,
        )
      }),
    [refresh, run],
  )

  const stopAll = useCallback(
    () =>
      run(async () => {
        const result = await networkApi.stopAll()
        await refresh()
        if (result.narrowed) {
          toast.success(`已停止 ${result.disabled} 条 Mock`)
        } else {
          toast.error('停止请求已发出，但浏览器尚未确认收窄')
        }
      }),
    [refresh, run],
  )

  return {
    status,
    records,
    rules,
    selected,
    selectedId,
    detailBody,
    select: setSelectedId,
    error,
    busy,
    refresh,
    startCapture,
    stopCapture,
    clearRecords,
    copyReference,
    deleteRecord,
    toggleRule,
    deleteRule,
    stopAll,
    fixNearMiss,
  }
}
