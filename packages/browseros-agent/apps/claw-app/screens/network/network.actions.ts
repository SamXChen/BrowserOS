/**
 * Mutations with their toasts, so the screen shell only wires components.
 */

import { toast } from 'sonner'
import { networkCopy } from './network.copy'
import type { NetworkScreenData } from './network.data'

const copy = networkCopy
const errorText = (cause: unknown) =>
  cause instanceof Error ? cause.message : String(cause)

export function useNetworkActions(data: NetworkScreenData) {
  const startCapture = (): void => {
    void data.actions.startCapture().then(
      () => toast.success(copy.capture.started),
      (cause: unknown) => toast.error(errorText(cause)),
    )
  }

  const stopCapture = (): void => {
    void data.actions.stopCapture().then(
      () => toast.success(copy.capture.stoppedToast),
      (cause: unknown) => toast.error(errorText(cause)),
    )
  }

  const stopAll = (): void => {
    void data.actions.stopAll().then(
      (result) => {
        if (!(result as { narrowed?: boolean }).narrowed) {
          toast.error(copy.errors.ruleAction('fetch_not_disabled'))
        }
      },
      (cause: unknown) => toast.error(errorText(cause)),
    )
  }

  const clearRecords = (done: () => void): void => {
    void data.actions.clearRecords().then(
      () => {
        done()
        toast.success(copy.capture.cleared)
      },
      (cause: unknown) => toast.error(errorText(cause)),
    )
  }

  const copyReference = (recordId: string): void => {
    void data.actions.copyReference(recordId).then(
      async (handoff) => {
        try {
          await navigator.clipboard.writeText(handoff.clipboardText)
          toast.success(copy.list.copied)
        } catch (cause) {
          toast.error(errorText(cause))
        }
      },
      (cause: unknown) => toast.error(errorText(cause)),
    )
  }

  const deleteRecord = (recordId: string, clearSelection: () => void): void => {
    void data.actions.deleteRecord(recordId).then(
      (result) => {
        const undoToken = (result as { undoToken?: string }).undoToken
        clearSelection()
        toast(copy.list.recordDeleted(1), {
          action: undoToken
            ? {
                label: copy.list.undo,
                onClick: () => {
                  void data.actions.restoreRecord(undoToken)
                },
              }
            : undefined,
        })
      },
      (cause: unknown) => toast.error(errorText(cause)),
    )
  }

  const deleteAllRules = (): void => {
    void data.actions.deleteAllRules().then(
      ({ ids }) => {
        toast(copy.rules.menu.deletedAll(ids.length), {
          action: {
            label: copy.list.undo,
            onClick: () => {
              void data.actions.restoreRules(ids).then(({ ok, failed }) => {
                if (failed > 0) toast(copy.rules.menu.restoredSome(ok, failed))
              })
            },
          },
        })
      },
      (cause: unknown) => toast.error(errorText(cause)),
    )
  }

  const copyForAi = (id: string, name: string, endpoint: string): void => {
    void navigator.clipboard
      .writeText(
        `规则「${name}」（${id}）拦截 ${endpoint}。请用 network 工具的 rule_apply 按名称修改它。`,
      )
      .then(
        () => toast(copy.rules.actions.copiedForAi),
        (cause: unknown) =>
          toast.error(copy.detail.copyFailed(errorText(cause))),
      )
  }

  return {
    startCapture,
    stopCapture,
    stopAll,
    clearRecords,
    copyReference,
    deleteRecord,
    deleteAllRules,
    copyForAi,
  }
}
