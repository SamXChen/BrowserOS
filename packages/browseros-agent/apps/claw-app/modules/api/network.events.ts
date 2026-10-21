/**
 * SSE client for `/api/v1/network/events`.
 *
 * Every (re)connect resolves the server base URL again, so a changed port is
 * picked up without a reload. Malformed frames are dropped instead of
 * breaking the screen.
 */

import type {
  NetworkCaptureState,
  NetworkHit,
  NetworkRecord,
  NetworkRule,
  NetworkStatus,
} from '@browseros/claw-api'
import { resolveApiBaseUrl } from './client'

export interface RecordDone {
  recordId: string
  bodyState?: string
  bodyReason?: string
}

/** Stream payload: `deleted` is not the REST DTO (that one is a result). */
export interface DeletedEvent {
  kind: 'record' | 'rule' | 'handoff'
  id?: string
  all?: boolean
  undoToken?: string
  undoUntil?: number
}

export type NetworkEvent =
  | { type: 'snapshot'; data: NetworkStatus }
  | { type: 'capture'; data: NetworkCaptureState }
  | { type: 'record'; data: { record: NetworkRecord } }
  | { type: 'record_done'; data: RecordDone }
  | { type: 'rule'; data: NetworkRule }
  | { type: 'hit'; data: NetworkHit }
  | { type: 'evicted'; data: { recordId: string } }
  | { type: 'deleted'; data: DeletedEvent }
  | { type: 'gap'; data: { reset?: boolean; lost?: number } }

export type ConnectionState = 'connecting' | 'open' | 'retrying'

/** The slice of `EventSource` this module needs, so tests can substitute one. */
export interface EventSourceLike {
  readonly readyState: number
  onerror: ((event: unknown) => void) | null
  addEventListener(
    type: string,
    listener: (event: { data?: string }) => void,
  ): void
  close(): void
}

export const RETRY_DELAYS_MS = [1000, 2000, 5000, 10_000] as const

const EVENT_TYPES = [
  'snapshot',
  'capture',
  'record',
  'record_done',
  'rule',
  'hit',
  'evicted',
  'deleted',
  'gap',
] as const

const CLOSED = 2
const CONNECTING = 0

export interface NetworkEventHandlers {
  onEvent(event: NetworkEvent): void
  onState(state: ConnectionState): void
  onOpen(): void
}

export interface NetworkEventOptions {
  /** Test seam: defaults to `resolveApiBaseUrl`. */
  resolveUrl?: () => Promise<string>
  /** Test seam: defaults to the global `EventSource`. */
  createSource?: (url: string) => EventSourceLike
  /** Test seam: defaults to `setTimeout`. */
  schedule?: (run: () => void, ms: number) => unknown
  cancel?: (handle: unknown) => void
}

export function connectNetworkEvents(
  handlers: NetworkEventHandlers,
  options: NetworkEventOptions = {},
): () => void {
  const resolveUrl = options.resolveUrl ?? resolveApiBaseUrl
  const createSource: (url: string) => EventSourceLike | null = (url) =>
    options.createSource?.(url) ??
    (new EventSource(url) as unknown as EventSourceLike)
  const schedule =
    options.schedule ??
    ((run: () => void, ms: number) => setTimeout(run, ms) as unknown)
  const cancel =
    options.cancel ?? ((handle: unknown) => clearTimeout(handle as number))

  let disposed = false
  let source: EventSourceLike | null = null
  let timer: unknown = null
  let attempt = 0
  let generation = 0

  const disconnect = (): void => {
    if (timer !== null) {
      cancel(timer)
      timer = null
    }
    source?.close()
    source = null
  }

  const connect = async (): Promise<void> => {
    const mine = ++generation
    let base: string
    try {
      base = await resolveUrl()
    } catch {
      base = ''
    }
    if (disposed || mine !== generation) return
    const url = `${base}/api/v1/network/events`
    handlers.onState('connecting')

    const next = createSource(url)
    if (!next) return
    source = next

    for (const type of EVENT_TYPES) {
      next.addEventListener(type, (event) => {
        if (typeof event.data !== 'string') return
        try {
          handlers.onEvent({
            type,
            data: JSON.parse(event.data),
          } as NetworkEvent)
        } catch {
          console.warn('[network] bad event', type)
        }
      })
    }

    next.addEventListener('open', () => {
      if (disposed) return
      attempt = 0
      handlers.onState('open')
      handlers.onOpen()
    })

    next.onerror = () => {
      if (disposed) return
      if (next.readyState === CONNECTING) {
        // The browser is already reconnecting this source itself.
        handlers.onState('connecting')
        return
      }
      if (next.readyState !== CLOSED) return
      source?.close()
      source = null
      handlers.onState('retrying')
      const delay =
        RETRY_DELAYS_MS[Math.min(attempt, RETRY_DELAYS_MS.length - 1)]
      attempt += 1
      timer = schedule(() => {
        timer = null
        void connect()
      }, delay)
    }
  }

  void connect()

  return () => {
    disposed = true
    generation += 1
    disconnect()
  }
}
