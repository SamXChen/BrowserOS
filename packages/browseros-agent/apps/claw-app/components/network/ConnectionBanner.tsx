import { useEffect, useState } from 'react'
import type { ConnectionState } from '@/modules/api/network.events'
import { networkCopy } from '@/screens/network/network.copy'

/**
 * Shows up only after the stream has been down for a moment, so a tab that is
 * opening its first connection does not flash a warning (U2). A connection
 * that never opened is reported by the error alert instead.
 */
export function ConnectionBanner({ state }: { state: ConnectionState }) {
  const [visible, setVisible] = useState(false)
  const [seconds, setSeconds] = useState(0)
  const [everOpen, setEverOpen] = useState(false)

  useEffect(() => {
    if (state === 'open') setEverOpen(true)
  }, [state])

  useEffect(() => {
    if (state === 'open' || !everOpen) {
      setVisible(false)
      return
    }
    const timer = setTimeout(() => setVisible(true), 3000)
    return () => clearTimeout(timer)
  }, [state, everOpen])

  useEffect(() => {
    if (state !== 'retrying') {
      setSeconds(0)
      return
    }
    setSeconds(10)
    const timer = setInterval(() => {
      setSeconds((current) => Math.max(0, current - 1))
    }, 1000)
    return () => clearInterval(timer)
  }, [state])

  if (!visible) return null

  return (
    <div
      aria-label={networkCopy.a11y.connectionBanner}
      className="flex h-8 items-center gap-2 rounded-md border border-amber-500/30 bg-amber-500/10 px-3 text-[12px] text-amber-700 dark:text-amber-400"
      role="status"
    >
      <span className="size-1.5 rounded-full bg-amber-500" />
      {networkCopy.connection.disconnected}
      {state === 'retrying' && seconds > 0
        ? networkCopy.connection.retryIn(seconds)
        : null}
    </div>
  )
}
