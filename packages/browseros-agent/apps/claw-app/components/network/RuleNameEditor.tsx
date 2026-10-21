import { useEffect, useReducer, useRef } from 'react'
import { ApiResponseError } from '@/modules/api/client'
import { networkCopy } from '@/screens/network/network.copy'

export interface NameEditorState {
  editing: boolean
  value: string
  error: string | null
}

export type NameEditorAction =
  | { type: 'start'; value: string }
  | { type: 'change'; value: string }
  | { type: 'cancel' }
  | { type: 'submit' }
  | { type: 'serverConflict' }
  | { type: 'failed'; message: string }

/** Client-side name rules, so an invalid name never needs a server round trip. */
export function validateRuleName(name: string): string | null {
  const value = name.trim()
  if (value.length < 1 || value.length > 60) {
    return networkCopy.rules.nameEditor.tooShort
  }
  // biome-ignore lint/suspicious/noControlCharactersInRegex: controlling is the point
  if (/[\u0000-\u001f\u007f]/.test(value)) {
    return networkCopy.rules.nameEditor.controlChars
  }
  if (/^(rule_|netref_)/i.test(value)) {
    return networkCopy.rules.nameEditor.reservedPrefix
  }
  return null
}

export function nameEditorReducer(
  state: NameEditorState,
  action: NameEditorAction,
): NameEditorState {
  switch (action.type) {
    case 'start':
      return { editing: true, value: action.value, error: null }
    case 'change':
      return { ...state, value: action.value, error: null }
    case 'cancel':
      return { editing: false, value: '', error: null }
    case 'submit': {
      const error = validateRuleName(state.value)
      if (error) return { ...state, error }
      return { ...state, editing: false, error: null }
    }
    case 'serverConflict':
      return { ...state, error: networkCopy.rules.nameEditor.conflict }
    case 'failed':
      return { ...state, error: action.message }
    default:
      return state
  }
}

export function RuleNameEditor({
  name,
  onRename,
  onDone,
}: {
  name: string
  onRename(next: string): Promise<unknown>
  onDone?(): void
}) {
  const [state, dispatch] = useReducer(nameEditorReducer, {
    editing: false,
    value: name,
    error: null,
  })
  const inputRef = useRef<HTMLInputElement>(null)
  const cancelled = useRef(false)

  useEffect(() => {
    if (!state.editing) return
    inputRef.current?.focus()
    inputRef.current?.select()
  }, [state.editing])

  const submit = async (): Promise<void> => {
    const error = validateRuleName(state.value)
    if (error) {
      dispatch({ type: 'submit' })
      return
    }
    const next = state.value.trim()
    if (next === name) {
      dispatch({ type: 'cancel' })
      onDone?.()
      return
    }
    try {
      await onRename(next)
      dispatch({ type: 'cancel' })
      onDone?.()
    } catch (cause) {
      if (cause instanceof ApiResponseError && cause.response.status === 409) {
        dispatch({ type: 'serverConflict' })
        return
      }
      dispatch({
        type: 'failed',
        message: cause instanceof Error ? cause.message : String(cause),
      })
    }
  }

  if (!state.editing) {
    return (
      <button
        aria-label={networkCopy.rules.actions.rename}
        className="group/title flex min-w-0 items-center gap-1 text-left font-semibold text-[13px]"
        onClick={() => dispatch({ type: 'start', value: name })}
        type="button"
      >
        <span className="truncate">{name}</span>
      </button>
    )
  }

  return (
    <span className="flex min-w-0 flex-col gap-0.5">
      <input
        aria-label={networkCopy.rules.actions.rename}
        className="h-7 w-full min-w-0 rounded border border-ledger-border bg-background px-1.5 font-semibold text-[13px]"
        onBlur={() => {
          if (cancelled.current) {
            cancelled.current = false
            return
          }
          void submit()
        }}
        onChange={(event) =>
          dispatch({ type: 'change', value: event.target.value })
        }
        onKeyDown={(event) => {
          if (event.key === 'Enter') {
            event.preventDefault()
            void submit()
          }
          if (event.key === 'Escape') {
            event.preventDefault()
            cancelled.current = true
            dispatch({ type: 'cancel' })
            onDone?.()
          }
        }}
        ref={inputRef}
        value={state.value}
      />
      {state.error ? (
        <span className="text-[11px] text-red-600 dark:text-red-400">
          {state.error}
        </span>
      ) : null}
    </span>
  )
}
