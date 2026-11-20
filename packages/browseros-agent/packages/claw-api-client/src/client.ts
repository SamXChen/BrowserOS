import type {
  ActivatePlanKeyRequest,
  AgentSettings,
  AppendRecordingEventsResponse,
  AuditAgentList,
  AuditCleanupResult,
  AuditRetention,
  AuditStorageState,
  CancelPlanRequest,
  CancelSessionResponse,
  ClaudeDesktopExtension,
  ClaudeDesktopInstallResult,
  CockpitStats,
  Connection,
  ConnectionList,
  FeedbackInvitation,
  HealthResponse,
  NetworkBodyPage,
  NetworkCaptureState,
  NetworkDeleted,
  NetworkHandoffCreated,
  NetworkHandoffDetail,
  NetworkHandoffList,
  NetworkHandoffRequest,
  NetworkHitList,
  NetworkHitsDeleted,
  NetworkProcessorTestResult,
  NetworkRecord,
  NetworkRecordList,
  NetworkRecordsCleared,
  NetworkRestoreRequest,
  NetworkRestoreResult,
  NetworkRuleDetail,
  NetworkRuleList,
  NetworkRulePatch,
  NetworkRuleRevisionList,
  NetworkRulesDeleted,
  NetworkRuleTestResult,
  NetworkRuleUpdated,
  NetworkStatus,
  NetworkStopAllResult,
  NetworkTabReloadRequest,
  Plan,
  PlanCheckout,
  PlanCheckoutStatus,
  PlanKey,
  PlanKeyChange,
  PlanPortal,
  PlanPortalRequest,
  PlanPricing,
  PlanSavings,
  RecordFeedbackInviteRequest,
  RecordingMetadata,
  ResolveHelpResponse,
  SessionDetail,
  SessionList,
  SessionScreenshotList,
  SetAuditRetentionRequest,
  ShutdownResponse,
  Skill,
  SkillCreate,
  SkillDetail,
  SkillFlowSource,
  SkillList,
  SkillRunList,
  SkillUpdate,
  StartPlanCheckoutRequest,
  SystemInfo,
  TelemetryState,
  UpdateAgentSettingsRequest,
} from '@browseros/claw-api'
import createClient, { type Client } from 'openapi-fetch'
import { ApiResponseError } from './errors'
import type { operations, paths } from './generated/openapi'
import type {
  GetSessionPreviewRequest,
  GetSessionScreenshotRequest,
} from './urls'

export type ApiFetcher = (
  input: Parameters<typeof globalThis.fetch>[0],
  init?: Parameters<typeof globalThis.fetch>[1],
) => ReturnType<typeof globalThis.fetch>

export interface ClawApiClientOptions {
  fetch?: ApiFetcher
  credentials?: RequestCredentials
}

export type ListSessionsRequest = NonNullable<
  operations['listSessions']['parameters']['query']
>

export interface UpdateTelemetryOperationRequest {
  updateTelemetryRequest: operations['updateTelemetry']['requestBody']['content']['application/json']
}

export type NetworkRecordIdRequest = NonNullable<
  NonNullable<paths['/api/v1/network/records/{recordId}']['get']>['parameters']
>['path']

export type NetworkHandoffIdRequest = NonNullable<
  NonNullable<paths['/api/v1/network/handoffs/{id}']['get']>['parameters']
>['path']

export type NetworkRuleIdRequest = NonNullable<
  NonNullable<paths['/api/v1/network/rules/{id}']['get']>['parameters']
>['path']

export type NetworkTabIdRequest = NonNullable<
  NonNullable<
    paths['/api/v1/network/tabs/{tabId}/reload']['post']
  >['parameters']
>['path']

export type NetworkBodyRequest = NetworkRecordIdRequest & {
  offset?: number
  limit?: number
}

function bodyQuery(request: NetworkBodyRequest): {
  offset?: number
  limit?: number
} {
  const query: { offset?: number; limit?: number } = {}
  if (request.offset !== undefined) query.offset = request.offset
  if (request.limit !== undefined) query.limit = request.limit
  return query
}

type RecordingHeaders =
  operations['appendRecordingEvents']['parameters']['header']

export interface AppendRecordingEventsRequest {
  xRecordingTabId: RecordingHeaders['X-Recording-Tab-Id']
  xRecordingDocumentId: RecordingHeaders['X-Recording-Document-Id']
  xRecordingBatchId: RecordingHeaders['X-Recording-Batch-Id']
  xRecordingHasGap?: RecordingHeaders['X-Recording-Has-Gap']
  body: operations['appendRecordingEvents']['requestBody']['content']['application/x-ndjson']
}

export type SessionRequest = operations['getSession']['parameters']['path']
export type HarnessRequest = operations['connectHarness']['parameters']['path']

export type SkillNameRequest = operations['getSkill']['parameters']['path']

export interface UpdateSkillRequest {
  name: SkillNameRequest['name']
  body: SkillUpdate
}

export interface ListSkillRunsRequest {
  name: SkillNameRequest['name']
  cursor?: number
  limit?: number
}

export type PlanCheckoutRequest =
  operations['getPlanCheckout']['parameters']['path']

export type PlanComputerRequest =
  operations['removePlanComputer']['parameters']['path']

export interface RenamePlanComputerOperationRequest {
  installId: PlanComputerRequest['installId']
  name: string
}

type ApiResult<T> =
  | { data: T; response: Response }
  | { error: unknown; response: Response }

export class ClawApiClient {
  private readonly client: Client<paths>
  private readonly responseClones = new WeakMap<Response, Response>()

  constructor(baseUrl: string, options: ClawApiClientOptions = {}) {
    const fetcher = options.fetch ?? globalThis.fetch
    const credentials = options.credentials ?? 'omit'
    const receiverSafeFetch = async (
      input: Parameters<typeof globalThis.fetch>[0],
      init?: Parameters<typeof globalThis.fetch>[1],
    ): Promise<Response> => {
      const response = await fetcher(input, {
        ...init,
        credentials: init?.credentials ?? credentials,
      })
      if (!response.ok) this.responseClones.set(response, response.clone())
      return response
    }
    this.client = createClient<paths>({
      baseUrl: baseUrl.replace(/\/+$/, ''),
      credentials,
      fetch: receiverSafeFetch,
    })
  }

  async getHealth(): Promise<HealthResponse> {
    return this.unwrap(await this.client.GET('/system/health'))
  }

  async shutdown(): Promise<ShutdownResponse> {
    return this.unwrap(await this.client.POST('/system/shutdown'))
  }

  async getSystemInfo(): Promise<SystemInfo> {
    return this.unwrap(await this.client.GET('/api/v1/system'))
  }

  async getCockpitStats(): Promise<CockpitStats> {
    return this.unwrap(await this.client.GET('/api/v1/cockpit/stats'))
  }

  async getFeedbackInvitation(): Promise<FeedbackInvitation> {
    return this.unwrap(await this.client.GET('/api/v1/feedback/invitation'))
  }

  async recordFeedbackInvite(
    body: RecordFeedbackInviteRequest,
  ): Promise<FeedbackInvitation> {
    return this.unwrap(
      await this.client.POST('/api/v1/feedback/invitation', { body }),
    )
  }

  async getTelemetry(): Promise<TelemetryState> {
    return this.unwrap(await this.client.GET('/api/v1/settings/telemetry'))
  }

  async getAgentSettings(): Promise<AgentSettings> {
    return this.unwrap(await this.client.GET('/api/v1/settings/agent'))
  }

  async updateAgentSettings(
    body: UpdateAgentSettingsRequest,
  ): Promise<AgentSettings> {
    return this.unwrap(
      await this.client.PUT('/api/v1/settings/agent', { body }),
    )
  }

  async getAuditStorage(): Promise<AuditStorageState> {
    return this.unwrap(await this.client.GET('/api/v1/audit/storage'))
  }

  async setAuditRetention(
    body: SetAuditRetentionRequest,
  ): Promise<AuditRetention> {
    return this.unwrap(
      await this.client.PUT('/api/v1/audit/retention', { body }),
    )
  }

  async runAuditCleanup(): Promise<AuditCleanupResult> {
    return this.unwrap(await this.client.POST('/api/v1/audit/cleanup'))
  }

  async listAuditAgents(): Promise<AuditAgentList> {
    return this.unwrap(await this.client.GET('/api/v1/audit/agents'))
  }

  async updateTelemetry(
    request: UpdateTelemetryOperationRequest,
  ): Promise<TelemetryState> {
    return this.unwrap(
      await this.client.PUT('/api/v1/settings/telemetry', {
        body: request.updateTelemetryRequest,
      }),
    )
  }

  async listSessions(request: ListSessionsRequest = {}): Promise<SessionList> {
    return this.unwrap(
      await this.client.GET('/api/v1/sessions', {
        params: { query: request },
      }),
    )
  }

  async getSession(request: SessionRequest): Promise<SessionDetail> {
    return this.unwrap(
      await this.client.GET('/api/v1/sessions/{sessionId}', {
        params: { path: request },
      }),
    )
  }

  async cancelSession(request: SessionRequest): Promise<CancelSessionResponse> {
    return this.unwrap(
      await this.client.POST('/api/v1/sessions/{sessionId}/cancel', {
        params: { path: request },
      }),
    )
  }

  async resolveHelp(
    request: SessionRequest & { note?: string },
  ): Promise<ResolveHelpResponse> {
    return this.unwrap(
      await this.client.POST('/api/v1/sessions/{sessionId}/help/resolve', {
        params: { path: { sessionId: request.sessionId } },
        body: { note: request.note },
      }),
    )
  }

  async getRecording(request: SessionRequest): Promise<RecordingMetadata> {
    return this.unwrap(
      await this.client.GET('/api/v1/sessions/{sessionId}/recording', {
        params: { path: request },
      }),
    )
  }

  async downloadRecordingEvents(request: SessionRequest): Promise<string> {
    return this.unwrap(
      await this.client.GET('/api/v1/sessions/{sessionId}/recording/events', {
        params: { path: request },
        parseAs: 'text',
      }),
    )
  }

  async appendRecordingEvents(
    request: AppendRecordingEventsRequest,
  ): Promise<AppendRecordingEventsResponse> {
    return this.unwrap(
      await this.client.POST('/api/v1/recordings/events', {
        params: {
          header: {
            'X-Recording-Tab-Id': request.xRecordingTabId,
            'X-Recording-Document-Id': request.xRecordingDocumentId,
            'X-Recording-Batch-Id': request.xRecordingBatchId,
            'X-Recording-Has-Gap': request.xRecordingHasGap,
          },
        },
        body: request.body,
        bodySerializer: (body) => body,
        headers: { 'content-type': 'application/x-ndjson' },
      }),
    )
  }

  async getSessionPreview(request: GetSessionPreviewRequest): Promise<Blob> {
    return this.unwrap(
      await this.client.GET('/api/v1/sessions/{sessionId}/preview', {
        params: {
          path: { sessionId: request.sessionId },
          query: {
            refresh: request.refresh,
            browserTabId: request.browserTabId,
          },
        },
        parseAs: 'blob',
      }),
    )
  }

  async listSessionScreenshots(
    request: SessionRequest,
  ): Promise<SessionScreenshotList> {
    return this.unwrap(
      await this.client.GET('/api/v1/sessions/{sessionId}/screenshots', {
        params: { path: request },
      }),
    )
  }

  async getSessionScreenshot(
    request: GetSessionScreenshotRequest,
  ): Promise<Blob> {
    return this.unwrap(
      await this.client.GET(
        '/api/v1/sessions/{sessionId}/screenshots/{screenshotId}',
        {
          params: { path: request },
          parseAs: 'blob',
        },
      ),
    )
  }

  async listConnections(): Promise<ConnectionList> {
    return this.unwrap(await this.client.GET('/api/v1/connections'))
  }

  async connectHarness(request: HarnessRequest): Promise<Connection> {
    return this.unwrap(
      await this.client.PUT('/api/v1/connections/{harness}', {
        params: { path: request },
      }),
    )
  }

  async disconnectHarness(request: HarnessRequest): Promise<Connection> {
    return this.unwrap(
      await this.client.DELETE('/api/v1/connections/{harness}', {
        params: { path: request },
      }),
    )
  }

  async getClaudeDesktopExtension(): Promise<ClaudeDesktopExtension> {
    return this.unwrap(
      await this.client.GET('/api/v1/claude-desktop/extension'),
    )
  }

  async installClaudeDesktopExtension(): Promise<ClaudeDesktopInstallResult> {
    return this.unwrap(
      await this.client.POST('/api/v1/claude-desktop/extension/install'),
    )
  }

  // ---- network mock ----------------------------------------------------

  async getNetworkState(): Promise<NetworkStatus> {
    return this.unwrap(await this.client.GET('/api/v1/network/state'))
  }

  async startNetworkCapture(): Promise<NetworkCaptureState> {
    return this.unwrap(await this.client.POST('/api/v1/network/captures'))
  }

  async stopNetworkCapture(): Promise<NetworkCaptureState> {
    return this.unwrap(await this.client.DELETE('/api/v1/network/captures'))
  }

  async listNetworkRecords(request?: {
    since?: number
    limit?: number
  }): Promise<NetworkRecordList> {
    return this.unwrap(
      await this.client.GET('/api/v1/network/records', {
        params: { query: request ?? {} },
      }),
    )
  }

  async deleteNetworkRecords(): Promise<NetworkRecordsCleared> {
    return this.unwrap(await this.client.DELETE('/api/v1/network/records'))
  }

  async getNetworkRecord(
    request: NetworkRecordIdRequest,
  ): Promise<NetworkRecord> {
    return this.unwrap(
      await this.client.GET('/api/v1/network/records/{recordId}', {
        params: { path: request },
      }),
    )
  }

  async deleteNetworkRecord(
    request: NetworkRecordIdRequest,
  ): Promise<NetworkDeleted> {
    return this.unwrap(
      await this.client.DELETE('/api/v1/network/records/{recordId}', {
        params: { path: request },
      }),
    )
  }

  async restoreNetworkRecord(
    body: NetworkRestoreRequest,
  ): Promise<NetworkRestoreResult> {
    return this.unwrap(
      await this.client.POST('/api/v1/network/records/restore', { body }),
    )
  }

  async getNetworkRequestBody(
    request: NetworkBodyRequest,
  ): Promise<NetworkBodyPage> {
    return this.unwrap(
      await this.client.GET('/api/v1/network/records/{recordId}/request-body', {
        params: { path: request, query: bodyQuery(request) },
      }),
    )
  }

  async getNetworkResponseBody(
    request: NetworkBodyRequest,
  ): Promise<NetworkBodyPage> {
    return this.unwrap(
      await this.client.GET(
        '/api/v1/network/records/{recordId}/response-body',
        {
          params: { path: request, query: bodyQuery(request) },
        },
      ),
    )
  }

  async listNetworkHandoffs(): Promise<NetworkHandoffList> {
    return this.unwrap(await this.client.GET('/api/v1/network/handoffs'))
  }

  async createNetworkHandoff(
    body: NetworkHandoffRequest,
  ): Promise<NetworkHandoffCreated> {
    return this.unwrap(
      await this.client.POST('/api/v1/network/handoffs', { body }),
    )
  }

  async getNetworkHandoff(
    request: NetworkHandoffIdRequest,
  ): Promise<NetworkHandoffDetail> {
    return this.unwrap(
      await this.client.GET('/api/v1/network/handoffs/{id}', {
        params: { path: request },
      }),
    )
  }

  async deleteNetworkHandoff(
    request: NetworkHandoffIdRequest,
  ): Promise<NetworkDeleted> {
    return this.unwrap(
      await this.client.DELETE('/api/v1/network/handoffs/{id}', {
        params: { path: request },
      }),
    )
  }

  async listNetworkRules(): Promise<NetworkRuleList> {
    return this.unwrap(await this.client.GET('/api/v1/network/rules'))
  }

  async deleteAllNetworkRules(): Promise<NetworkRulesDeleted> {
    return this.unwrap(await this.client.DELETE('/api/v1/network/rules'))
  }

  async getNetworkRule(
    request: NetworkRuleIdRequest,
  ): Promise<NetworkRuleDetail> {
    return this.unwrap(
      await this.client.GET('/api/v1/network/rules/{id}', {
        params: { path: request },
      }),
    )
  }

  async patchNetworkRule(
    request: NetworkRuleIdRequest,
    body: NetworkRulePatch,
  ): Promise<NetworkRuleUpdated> {
    return this.unwrap(
      await this.client.PATCH('/api/v1/network/rules/{id}', {
        params: { path: request },
        body,
      }),
    )
  }

  async deleteNetworkRule(
    request: NetworkRuleIdRequest,
  ): Promise<NetworkDeleted> {
    return this.unwrap(
      await this.client.DELETE('/api/v1/network/rules/{id}', {
        params: { path: request },
      }),
    )
  }

  async restoreNetworkRule(
    request: NetworkRuleIdRequest,
  ): Promise<NetworkRestoreResult> {
    return this.unwrap(
      await this.client.POST('/api/v1/network/rules/{id}/restore', {
        params: { path: request },
      }),
    )
  }

  async testNetworkRule(
    request: NetworkRuleIdRequest,
  ): Promise<NetworkRuleTestResult> {
    return this.unwrap(
      await this.client.POST('/api/v1/network/rules/{id}/test', {
        params: { path: request },
      }),
    )
  }

  async testNetworkProcessor(
    request: NetworkRuleIdRequest,
  ): Promise<NetworkProcessorTestResult> {
    return this.unwrap(
      await this.client.POST('/api/v1/network/rules/{id}/processor-test', {
        params: { path: request },
        body: {},
      }),
    )
  }

  async listNetworkRuleRevisions(
    request: NetworkRuleIdRequest,
  ): Promise<NetworkRuleRevisionList> {
    return this.unwrap(
      await this.client.GET('/api/v1/network/rules/{id}/revisions', {
        params: { path: request },
      }),
    )
  }

  async restoreNetworkRuleRevision(request: {
    id: string
    revision: number
  }): Promise<NetworkRuleUpdated> {
    return this.unwrap(
      await this.client.POST(
        '/api/v1/network/rules/{id}/revisions/{revision}/restore',
        {
          params: { path: request },
        },
      ),
    )
  }

  async reloadNetworkTab(
    request: NetworkTabIdRequest,
    body: NetworkTabReloadRequest,
  ): Promise<void> {
    return this.unwrap(
      await this.client.POST('/api/v1/network/tabs/{tabId}/reload', {
        params: { path: request },
        body,
      }),
    )
  }

  async stopAllNetworkMocks(): Promise<NetworkStopAllResult> {
    return this.unwrap(await this.client.POST('/api/v1/network/stop-all'))
  }

  async listNetworkHits(request?: {
    ruleId?: string
    limit?: number
  }): Promise<NetworkHitList> {
    return this.unwrap(
      await this.client.GET('/api/v1/network/hits', {
        params: { query: request ?? {} },
      }),
    )
  }

  async deleteNetworkHits(request: {
    ruleId: string
  }): Promise<NetworkHitsDeleted> {
    return this.unwrap(
      await this.client.DELETE('/api/v1/network/hits', {
        params: { query: request },
      }),
    )
  }

  async listSkills(): Promise<SkillList> {
    return this.unwrap(await this.client.GET('/api/v1/skills'))
  }

  async getSkill(request: SkillNameRequest): Promise<SkillDetail> {
    return this.unwrap(
      await this.client.GET('/api/v1/skills/{name}', {
        params: { path: request },
      }),
    )
  }

  async createSkill(body: SkillCreate): Promise<Skill> {
    return this.unwrap(await this.client.POST('/api/v1/skills', { body }))
  }

  async updateSkill(request: UpdateSkillRequest): Promise<SkillDetail> {
    return this.unwrap(
      await this.client.PUT('/api/v1/skills/{name}', {
        params: { path: { name: request.name } },
        body: request.body,
      }),
    )
  }

  async deleteSkill(request: SkillNameRequest): Promise<void> {
    this.expectSuccess(
      await this.client.DELETE('/api/v1/skills/{name}', {
        params: { path: request },
      }),
    )
  }

  async listSkillRuns(request: ListSkillRunsRequest): Promise<SkillRunList> {
    const { name, ...query } = request
    return this.unwrap(
      await this.client.GET('/api/v1/skills/{name}/runs', {
        params: { path: { name }, query },
      }),
    )
  }

  async getSkillFlow(request: SkillNameRequest): Promise<SkillFlowSource> {
    return this.unwrap(
      await this.client.GET('/api/v1/skills/{name}/flow', {
        params: { path: request },
      }),
    )
  }

  async approveSkillWrite(request: SkillNameRequest): Promise<Skill> {
    return this.unwrap(
      await this.client.POST('/api/v1/skills/{name}/ultrafast/approve-write', {
        params: { path: request },
      }),
    )
  }

  async revokeSkillWrite(request: SkillNameRequest): Promise<Skill> {
    return this.unwrap(
      await this.client.DELETE(
        '/api/v1/skills/{name}/ultrafast/approve-write',
        {
          params: { path: request },
        },
      ),
    )
  }

  async getPlan(): Promise<Plan> {
    return this.unwrap(await this.client.GET('/api/v1/plan'))
  }

  async getPlanPricing(): Promise<PlanPricing> {
    return this.unwrap(await this.client.GET('/api/v1/plan/pricing'))
  }

  async getPlanSavings(): Promise<PlanSavings> {
    return this.unwrap(await this.client.GET('/api/v1/plan/savings'))
  }

  async startPlanCheckout(
    body: StartPlanCheckoutRequest,
  ): Promise<PlanCheckout> {
    return this.unwrap(
      await this.client.POST('/api/v1/plan/checkout', { body }),
    )
  }

  async getPlanCheckout(
    request: PlanCheckoutRequest,
  ): Promise<PlanCheckoutStatus> {
    return this.unwrap(
      await this.client.GET('/api/v1/plan/checkout/{checkoutId}', {
        params: { path: request },
      }),
    )
  }

  async getPlanKey(): Promise<PlanKey> {
    return this.unwrap(await this.client.GET('/api/v1/plan/key'))
  }

  /** The key is sent once and never comes back in any reply. */
  async activatePlanKey(body: ActivatePlanKeyRequest): Promise<PlanKeyChange> {
    return this.unwrap(
      await this.client.POST('/api/v1/plan/key/activate', { body }),
    )
  }

  async removePlanComputer(
    request: PlanComputerRequest,
  ): Promise<PlanKeyChange> {
    return this.unwrap(
      await this.client.POST('/api/v1/plan/key/computers/{installId}/remove', {
        params: { path: request },
      }),
    )
  }

  async renamePlanComputer(
    request: RenamePlanComputerOperationRequest,
  ): Promise<PlanKey> {
    return this.unwrap(
      await this.client.POST('/api/v1/plan/key/computers/{installId}/rename', {
        params: { path: { installId: request.installId } },
        body: { name: request.name },
      }),
    )
  }

  /** The new key is emailed; the reply carries only its hint. */
  async resetPlanKey(): Promise<PlanKeyChange> {
    return this.unwrap(await this.client.POST('/api/v1/plan/key/reset'))
  }

  async cancelPlan(body: CancelPlanRequest = {}): Promise<Plan> {
    return this.unwrap(await this.client.POST('/api/v1/plan/cancel', { body }))
  }

  async resumePlan(): Promise<Plan> {
    return this.unwrap(await this.client.POST('/api/v1/plan/resume'))
  }

  async openPlanPortal(body: PlanPortalRequest): Promise<PlanPortal> {
    return this.unwrap(await this.client.POST('/api/v1/plan/portal', { body }))
  }

  async refreshPlan(): Promise<Plan> {
    return this.unwrap(await this.client.POST('/api/v1/plan/refresh'))
  }

  private unwrap<T>(result: ApiResult<T | undefined>): T {
    const errorResponse = this.responseClones.get(result.response)
    this.responseClones.delete(result.response)
    if ('error' in result) {
      throw new ApiResponseError(errorResponse ?? result.response, result.error)
    }
    if (result.data === undefined) {
      throw new Error('NEOB API returned an empty success response')
    }
    return result.data
  }

  // A 204 response carries no body, so this only surfaces a failure.
  private expectSuccess(result: { error?: unknown; response: Response }): void {
    const errorResponse = this.responseClones.get(result.response)
    this.responseClones.delete(result.response)
    if ('error' in result) {
      throw new ApiResponseError(errorResponse ?? result.response, result.error)
    }
  }
}
