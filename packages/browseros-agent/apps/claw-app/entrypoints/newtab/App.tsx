import { DiagnosticsPage } from '@browseros/diagnostics/view'
import type { ReactNode } from 'react'
import { HashRouter, Navigate, Route, Routes } from 'react-router'
import { CockpitShell } from '@/components/layout/CockpitShell'
import { Page } from '@/components/layout/Page'
import { Analytics } from '@/modules/analytics/Analytics'
import { Audit } from '@/screens/audit/Audit'
import { BlankNewTab } from '@/screens/blank/BlankNewTab'
import { Cockpit } from '@/screens/cockpit/Cockpit'
import { Mcp } from '@/screens/mcp/Mcp'
import { Network } from '@/screens/network/Network'
import { Pro } from '@/screens/pro/Pro'
import { Settings } from '@/screens/settings/Settings'
import { SkillDetail } from '@/screens/skills/SkillDetail'
import { Skills } from '@/screens/skills/Skills'
import { TaskDetailPage } from '@/screens/task-detail/TaskDetailPage'

/** Mounts the v2 cockpit route tree. */
export function App() {
  return (
    <HashRouter>
      <Analytics />
      <Routes>
        {/* The new tab page is intentionally blank; the cockpit lives one
            click away at /cockpit and must not share its sidebar. */}
        <Route path="/" element={<BlankNewTab />} />
        <Route element={<CockpitShell />}>
          <Route path="/cockpit" element={<Cockpit />} />
          <Route path="/mcp" element={<Mcp />} />
          <Route
            path="/diagnostics"
            element={
              <Page>
                <DiagnosticsPage />
              </Page>
            }
          />
          <Route path="/skills" element={<Skills />} />
          <Route path="/skills/:name" element={<SkillDetail />} />
          <Route path="/pro" element={<Pro />} />
          <Route path="/ultrafast" element={<Navigate to="/pro" replace />} />
          <Route path="/settings" element={<Settings />} />
          <Route
            path="/network"
            element={
              <SensitiveRouteContent>
                <Network />
              </SensitiveRouteContent>
            }
          />
          <Route
            path="/audit"
            element={
              <SensitiveRouteContent>
                <Audit />
              </SensitiveRouteContent>
            }
          />
          <Route
            path="/audit/:sessionId"
            element={
              <SensitiveRouteContent>
                <TaskDetailPage />
              </SensitiveRouteContent>
            }
          />
          <Route
            path="/audit/:sessionId/replay"
            element={
              <SensitiveRouteContent>
                <TaskDetailPage focus="player" />
              </SensitiveRouteContent>
            }
          />
        </Route>
        <Route path="*" element={<Navigate to="/cockpit" replace />} />
      </Routes>
    </HashRouter>
  )
}

/** Blocks task-bearing route bodies from PostHog session snapshots. */
export function SensitiveRouteContent({ children }: { children: ReactNode }) {
  return <div className="ph-no-capture contents">{children}</div>
}
