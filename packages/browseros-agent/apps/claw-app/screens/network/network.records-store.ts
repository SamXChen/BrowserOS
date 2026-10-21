/**
 * External store for captured requests.
 *
 * Records arrive in SSE bursts, so events are coalesced into one update per
 * frame; the snapshot object only changes when something actually moved, which
 * keeps `useSyncExternalStore` cheap.
 */

import type { NetworkRecord } from '@browseros/claw-api'
import type {
  DeletedEvent,
  NetworkEvent,
  RecordDone,
} from '@/modules/api/network.events'
import { type DerivedRecord, recordDerived } from './network.helpers'

export interface RecordsSnapshot {
  order: string[]
  byId: ReadonlyMap<string, DerivedRecord>
  version: number
}

const EMPTY: RecordsSnapshot = {
  order: [],
  byId: new Map(),
  version: 0,
}

const STATE_RANK: Record<string, number> = {
  created: 0,
  response_ready: 1,
  done: 2,
  failed: 2,
}

const BODY_FINAL = new Set(['available', 'too_large', 'unavailable'])

function rank(state: string | undefined): number {
  return state ? (STATE_RANK[state] ?? 0) : 0
}

/**
 * Merges one update into an existing record. Monotonic: a delayed event can
 * never downgrade the state or the body availability already reported.
 */
export function mergeRecord(
  existing: DerivedRecord | undefined,
  incoming: NetworkRecord,
): DerivedRecord {
  if (!existing) {
    return { ...incoming, derived: recordDerived(incoming) }
  }
  const nextState =
    rank(incoming.state) >= rank(existing.state)
      ? incoming.state
      : existing.state
  const incomingBody = incoming.bodyState
  const bodyState =
    existing.bodyState && BODY_FINAL.has(existing.bodyState)
      ? existing.bodyState
      : (incomingBody ?? existing.bodyState)

  const merged = {
    ...existing,
    ...Object.fromEntries(
      Object.entries(incoming).filter(([, value]) => value !== undefined),
    ),
    state: nextState,
    bodyState,
    mock: incoming.mock ?? existing.mock,
  } as DerivedRecord
  if (incoming.url !== existing.url) {
    merged.derived = recordDerived(merged)
  }
  return merged
}

function removeFrom(
  id: string,
  order: string[],
  byId: Map<string, DerivedRecord>,
): boolean {
  if (!byId.delete(id)) return false
  const index = order.indexOf(id)
  if (index !== -1) order.splice(index, 1)
  return true
}

function applyRecord(
  record: NetworkRecord | undefined,
  order: string[],
  byId: Map<string, DerivedRecord>,
  limit: number,
): boolean {
  if (!record) return false
  byId.set(record.recordId, mergeRecord(byId.get(record.recordId), record))
  const index = order.indexOf(record.recordId)
  if (index !== -1) order.splice(index, 1)
  order.unshift(record.recordId)
  while (order.length > limit) {
    const dropped = order.pop()
    if (dropped) byId.delete(dropped)
  }
  return true
}

function applyRecordDone(
  data: RecordDone | undefined,
  byId: Map<string, DerivedRecord>,
): boolean {
  const id = data?.recordId
  const existing = id ? byId.get(id) : undefined
  if (!id || !existing) return false
  byId.set(
    id,
    mergeRecord(existing, {
      ...existing,
      bodyState: data?.bodyState ?? existing.bodyState,
      bodyReason: data?.bodyReason ?? existing.bodyReason,
    }),
  )
  return true
}

function applyEviction(
  id: string | undefined,
  order: string[],
  byId: Map<string, DerivedRecord>,
): boolean {
  return id ? removeFrom(id, order, byId) : false
}

function applyDeletion(
  deletion: DeletedEvent | undefined,
  order: string[],
  byId: Map<string, DerivedRecord>,
): boolean {
  if (deletion?.kind !== 'record') return false
  if (deletion.all) {
    if (order.length === 0) return false
    order.length = 0
    byId.clear()
    return true
  }
  return deletion.id ? removeFrom(deletion.id, order, byId) : false
}

export interface RecordsStore {
  getSnapshot(): RecordsSnapshot
  subscribe(listener: () => void): () => void
  /** Replaces everything; buffered events are replayed on top (they only merge). */
  replaceAll(records: NetworkRecord[]): void
  enqueue(event: NetworkEvent): void
  clear(): void
}

export function createRecordsStore(limit = 500): RecordsStore {
  let snapshot: RecordsSnapshot = EMPTY
  let buffered: NetworkEvent[] = []
  let frame: unknown = null
  const listeners = new Set<() => void>()

  const notify = (): void => {
    for (const listener of listeners) listener()
  }

  const publish = (order: string[], byId: Map<string, DerivedRecord>): void => {
    snapshot = { order, byId, version: snapshot.version + 1 }
    notify()
  }

  const applyEvent = (event: NetworkEvent): boolean => {
    const order = snapshot.order.slice()
    const byId = new Map(snapshot.byId)
    const changed =
      event.type === 'record'
        ? applyRecord(event.data?.record, order, byId, limit)
        : event.type === 'record_done'
          ? applyRecordDone(event.data, byId)
          : event.type === 'evicted'
            ? applyEviction(event.data?.recordId, order, byId)
            : event.type === 'deleted'
              ? applyDeletion(event.data, order, byId)
              : false
    if (changed) publish(order, byId)
    return changed
  }

  const flush = (): void => {
    frame = null
    const events = buffered
    buffered = []
    for (const event of events) applyEvent(event)
  }

  const schedule = (): void => {
    if (frame !== null) return
    const run = () => flush()
    frame =
      typeof document !== 'undefined' && document.hidden
        ? setTimeout(run, 16)
        : typeof requestAnimationFrame === 'function'
          ? requestAnimationFrame(run)
          : setTimeout(run, 16)
  }

  return {
    getSnapshot: () => snapshot,
    subscribe(listener) {
      listeners.add(listener)
      return () => listeners.delete(listener)
    },
    replaceAll(records) {
      const sorted = records
        .slice()
        .sort((left, right) => right.seq - left.seq)
        .slice(0, limit)
      const byId = new Map<string, DerivedRecord>()
      for (const record of sorted)
        byId.set(record.recordId, mergeRecord(undefined, record))
      snapshot = {
        order: sorted.map((record) => record.recordId),
        byId,
        version: snapshot.version + 1,
      }
      notify()
      // Events that arrived during the fetch are newer than the snapshot's
      // data in the merge order, and merging is monotonic.
      if (buffered.length > 0) flush()
    },
    enqueue(event) {
      buffered.push(event)
      schedule()
    },
    clear() {
      buffered = []
      if (frame !== null) {
        if (typeof cancelAnimationFrame === 'function')
          cancelAnimationFrame(frame as number)
        else clearTimeout(frame as number)
        frame = null
      }
      if (snapshot.order.length === 0) return
      publish([], new Map())
    },
  }
}
