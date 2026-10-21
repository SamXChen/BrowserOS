import { describe, expect, it } from 'bun:test'
import type { NetworkRecord } from '@browseros/claw-api'
import type { NetworkEvent } from '@/modules/api/network.events'
import { createRecordsStore, mergeRecord } from './network.records-store'

function record(overrides: Partial<NetworkRecord> = {}): NetworkRecord {
  return {
    recordId: 'netrec_1',
    seq: 1,
    requestId: 'req_1',
    method: 'GET',
    url: 'https://shop.test/api/items',
    startedAtMs: 1_000,
    state: 'created',
    bodyState: 'pending',
    ...overrides,
  }
}

const recordEvent = (incoming: NetworkRecord): NetworkEvent => ({
  type: 'record',
  data: { record: incoming },
})

const nextFrame = () => new Promise((resolve) => setTimeout(resolve, 40))

describe('mergeRecord', () => {
  it('never regresses state or a finished body state', () => {
    const done = mergeRecord(
      undefined,
      record({ state: 'done', bodyState: 'available' }),
    )
    const delayed = mergeRecord(
      done,
      record({ state: 'response_ready', bodyState: 'pending' }),
    )
    expect(delayed.state).toBe('done')
    expect(delayed.bodyState).toBe('available')

    const failed = mergeRecord(delayed, record({ state: 'failed' }))
    expect(failed.state).toBe('failed')
    expect(failed.bodyState).toBe('available')
  })

  it('keeps the mock mark and ignores undefined updates', () => {
    const existing = mergeRecord(
      undefined,
      record({ mock: { ruleId: 'rule_1', revision: 1, outcome: 'fulfilled' } }),
    )
    const merged = mergeRecord(existing, record({ status: 200 }))
    expect(merged.mock?.ruleId).toBe('rule_1')
    expect(merged.status).toBe(200)
  })
})

describe('createRecordsStore', () => {
  it('replaces untouched records by identity', async () => {
    const store = createRecordsStore()
    store.replaceAll([
      record({ recordId: 'a', seq: 1 }),
      record({ recordId: 'b', seq: 2 }),
    ])
    const before = store.getSnapshot()
    const untouched = before.byId.get('b')

    store.enqueue(recordEvent(record({ recordId: 'a', seq: 1, status: 200 })))
    await nextFrame()

    const after = store.getSnapshot()
    expect(after.byId.get('b')).toBe(untouched as never)
    expect(after.byId.get('a')?.status).toBe(200)
  })

  it('replays events buffered while a resync was in flight', async () => {
    const store = createRecordsStore()
    store.replaceAll([record({ recordId: 'a', seq: 1, state: 'created' })])

    // The event lands after the GET was issued but before `replaceAll` runs.
    store.enqueue(
      recordEvent(
        record({ recordId: 'a', seq: 1, state: 'done', status: 200 }),
      ),
    )
    store.replaceAll([record({ recordId: 'a', seq: 1, state: 'created' })])
    await nextFrame()

    expect(store.getSnapshot().byId.get('a')?.state).toBe('done')
    expect(store.getSnapshot().byId.get('a')?.status).toBe(200)
  })

  it('keeps the newest records only', async () => {
    const store = createRecordsStore(2)
    store.replaceAll([
      record({ recordId: 'old', seq: 1 }),
      record({ recordId: 'mid', seq: 2 }),
      record({ recordId: 'new', seq: 3 }),
    ])
    expect(store.getSnapshot().order).toEqual(['new', 'mid'])

    store.enqueue(recordEvent(record({ recordId: 'newer', seq: 4 })))
    await nextFrame()
    expect(store.getSnapshot().order).toEqual(['newer', 'new'])
  })

  it('keeps records when capture stops and only clears on demand', async () => {
    const store = createRecordsStore()
    store.replaceAll([
      record({ recordId: 'a' }),
      record({ recordId: 'b', seq: 2 }),
    ])
    // Stopping capture is a status change, not a store event.
    store.replaceAll([
      record({ recordId: 'a' }),
      record({ recordId: 'b', seq: 2 }),
    ])
    expect(store.getSnapshot().order).toHaveLength(2)

    store.clear()
    expect(store.getSnapshot().order).toHaveLength(0)
  })

  it('removes deleted and evicted records', async () => {
    const store = createRecordsStore()
    store.replaceAll([
      record({ recordId: 'a' }),
      record({ recordId: 'b', seq: 2 }),
    ])
    store.enqueue({ type: 'evicted', data: { recordId: 'a' } })
    await nextFrame()
    expect(store.getSnapshot().order).toEqual(['b'])

    store.enqueue({ type: 'deleted', data: { kind: 'record', all: true } })
    await nextFrame()
    expect(store.getSnapshot().order).toEqual([])
  })
})
