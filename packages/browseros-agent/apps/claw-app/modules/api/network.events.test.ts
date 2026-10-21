import { beforeEach, describe, expect, it, mock } from 'bun:test'
import {
  type ConnectionState,
  connectNetworkEvents,
  type EventSourceLike,
  type NetworkEvent,
} from './network.events'

class FakeSource implements EventSourceLike {
  static instances: FakeSource[] = []
  readyState = 0
  onerror: ((event: unknown) => void) | null = null
  private listeners = new Map<
    string,
    Array<(event: { data?: string }) => void>
  >()

  constructor(public url: string) {
    FakeSource.instances.push(this)
  }

  addEventListener(
    type: string,
    listener: (event: { data?: string }) => void,
  ): void {
    const list = this.listeners.get(type) ?? []
    list.push(listener)
    this.listeners.set(type, list)
  }

  close(): void {
    this.readyState = 2
  }

  emit(type: string, data?: string): void {
    for (const listener of this.listeners.get(type) ?? []) listener({ data })
  }

  open(): void {
    this.readyState = 1
    this.emit('open')
  }

  failClosed(): void {
    this.readyState = 2
    this.onerror?.({})
  }
}

interface Scheduled {
  run: () => void
  ms: number
}

function setup(resolveUrl: () => Promise<string>) {
  const states: ConnectionState[] = []
  const events: NetworkEvent[] = []
  const scheduled: Scheduled[] = []
  const opened: number[] = []
  const dispose = connectNetworkEvents(
    {
      onEvent: (event) => events.push(event),
      onState: (state) => states.push(state),
      onOpen: () => opened.push(Date.now()),
    },
    {
      resolveUrl,
      createSource: (url) => new FakeSource(url),
      schedule: (run, ms) => {
        scheduled.push({ run, ms })
        return scheduled.length
      },
      cancel: () => {},
    },
  )
  return { dispose, states, events, scheduled, opened }
}

const flush = () => new Promise((resolve) => setTimeout(resolve, 0))

beforeEach(() => {
  FakeSource.instances = []
})

describe('connectNetworkEvents', () => {
  it('reconnects with backoff after the source closes', async () => {
    const { states, scheduled } = setup(async () => 'http://a')
    await flush()
    expect(FakeSource.instances).toHaveLength(1)

    const first = FakeSource.instances[0]
    first.open()
    expect(states).toEqual(['connecting', 'open'])

    first.failClosed()
    expect(states.at(-1)).toBe('retrying')
    expect(scheduled.map((entry) => entry.ms)).toEqual([1000])
  })

  it('keeps backing off up to the ten second cap', async () => {
    const { scheduled } = setup(async () => 'http://a')
    await flush()

    for (let index = 0; index < 5; index += 1) {
      const source = FakeSource.instances.at(-1) as FakeSource
      source.failClosed()
      const next = scheduled.at(-1) as Scheduled
      next.run()
      await flush()
    }
    expect(scheduled.map((entry) => entry.ms)).toEqual([
      1000, 2000, 5000, 10_000, 10_000,
    ])
  })

  it('drops a malformed payload and warns instead of throwing', async () => {
    const warn = mock(() => {})
    const original = console.warn
    console.warn = warn as unknown as typeof console.warn
    try {
      const { events } = setup(async () => 'http://a')
      await flush()
      const source = FakeSource.instances[0]
      source.emit('rule', '{not json')
      source.emit('rule', JSON.stringify({ id: 'rule_1', name: 'ok' }))
      expect(warn).toHaveBeenCalledTimes(1)
      expect(events).toHaveLength(1)
      expect(events[0]?.type).toBe('rule')
      expect((events[0] as { data: { id: string } }).data.id).toBe('rule_1')
    } finally {
      console.warn = original
    }
  })

  it('resolves the base url again on every connect', async () => {
    let call = 0
    const { scheduled } = setup(async () => {
      call += 1
      return call === 1 ? 'http://a' : 'http://b'
    })
    await flush()
    expect(FakeSource.instances[0].url).toBe('http://a/api/v1/network/events')

    FakeSource.instances[0].failClosed()
    const next = scheduled.at(-1) as Scheduled
    next.run()
    await flush()
    expect(FakeSource.instances.at(-1)?.url).toBe(
      'http://b/api/v1/network/events',
    )
  })
})
