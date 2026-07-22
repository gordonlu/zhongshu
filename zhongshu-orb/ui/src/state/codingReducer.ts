import type { CodingUiEvent, OrganizationGraphView, OrganizationRecoveryResult, OrganizationUiEvent, OverlayToUiEvent, PatchDiffPayload } from '../ipc/events'

export type PlanStep = {
  id: string
  title: string
  status: string
}

export type WorkerState = {
  worker: string
  taskId: string
  status: 'assigned' | 'running' | 'reported' | 'submitted' | 'completed' | 'conflict'
  ownedFiles: string[]
  role?: string
  responsibility?: string
  reportsTo?: string
  reason?: string
}

export type ChangeState = {
  path: string
  operation: string
  summary: string
  diff?: PatchDiffPayload
  status: 'preview' | 'applied'
  changed?: boolean
}

export type VerificationState = {
  command: string
  success: boolean
  exitCode?: number
}

export type CodingState = {
  active: boolean
  sessionId?: string
  risk?: string
  planStepCount: number
  steps: PlanStep[]
  workers: WorkerState[]
  changes: ChangeState[]
  verifications: VerificationState[]
  recoveryMessages: string[]
  organizationGraphs: OrganizationGraphView[]
  organizationRecoveryResults: OrganizationRecoveryResult[]
  contextIncluded: {
    description: string
    estimatedTokens: number
  }[]
  phase?: {
    from: string
    to: string
  }
  contextPressure?: number
  droppedEvidence?: number
  droppedRecent?: number
  replay?: {
    conversationId?: number
    replayExecutionId?: string
  }
  organization?: {
    taskId: string
    manager: string
    collaboration: string
    status: string
    reason?: string
    handoff?: { from: string; to: string }
  }
  autoDelegation?: {
    routingId: string
    strategy: string
    reason: string
    workerCount: number
  }
}

export const initialCodingState: CodingState = {
  active: false,
  planStepCount: 0,
  steps: [],
  workers: [],
  changes: [],
  verifications: [],
  recoveryMessages: [],
  organizationGraphs: [],
  organizationRecoveryResults: [],
  contextIncluded: [],
}

export function codingReducer(state: CodingState, event: OverlayToUiEvent): CodingState {
  if (event.type === 'coding') return reduceCodingEvent(state, event.event)
  if (event.type === 'organization') return reduceOrganizationEvent(state, event.event)
  if (event.type === 'organization_graphs') {
    const hasUnfinishedGraph = event.graphs.some((view) => (
      view.graph.nodes.some((node) => !['succeeded', 'failed', 'skipped', 'cancelled'].includes(node.state))
    ))
    return { ...state, active: state.active || hasUnfinishedGraph, organizationGraphs: event.graphs }
  }
  if (event.type === 'organization_recovery') {
    return {
      ...state,
      active: true,
      organizationGraphs: upsertOrganizationGraph(state.organizationGraphs, event.result.graph),
      organizationRecoveryResults: [...state.organizationRecoveryResults, event.result],
    }
  }
  if (event.type === 'verification') {
    return {
      ...state,
      active: true,
      verifications: [
        ...state.verifications,
        {
          command: event.command,
          success: event.success,
          exitCode: event.exit_code,
        },
      ],
    }
  }
  if (event.type === 'recovery_feedback') {
    return {
      ...state,
      active: true,
      recoveryMessages: [...state.recoveryMessages, `${event.rule_id}: ${event.message}`],
    }
  }
  if (event.type === 'phase_transition') {
    return {
      ...state,
      active: true,
      phase: {
        from: event.from,
        to: event.to,
      },
    }
  }
  return state
}

function upsertOrganizationGraph(
  graphs: OrganizationGraphView[],
  next: OrganizationGraphView,
): OrganizationGraphView[] {
  const index = graphs.findIndex((view) => view.graph.task_id === next.graph.task_id)
  if (index === -1) return [...graphs, next]
  return graphs.map((view, viewIndex) => viewIndex === index ? next : view)
}

function reduceOrganizationEvent(state: CodingState, event: OrganizationUiEvent): CodingState {
  switch (event.kind) {
    case 'routing_decided':
      return {
        ...state,
        autoDelegation: {
          routingId: event.routing_id,
          strategy: event.strategy,
          reason: event.reason,
          workerCount: event.worker_count,
        },
      }
    case 'task_started':
      return {
        ...state,
        active: true,
        sessionId: event.task_id,
        workers: [],
        organization: {
          taskId: event.task_id,
          manager: event.manager,
          collaboration: event.collaboration,
          status: 'staffing',
        },
      }
    case 'employee_assigned':
      return {
        ...state,
        active: true,
        workers: upsertWorker(state.workers, {
          worker: event.employee,
          taskId: `${event.task_id}:${event.employee}`,
          status: 'assigned',
          ownedFiles: [],
          role: event.role,
          responsibility: event.responsibility,
          reportsTo: event.reports_to,
        }),
      }
    case 'employee_working':
      return {
        ...state,
        active: true,
        workers: updateOrganizationWorker(state.workers, event.employee, {
          status: 'running',
          role: event.role,
        }),
        organization: state.organization ? { ...state.organization, status: 'working' } : state.organization,
      }
    case 'employee_reported':
      const reportFailed = ['failed', 'blocked', 'interrupted'].includes(event.outcome)
      return {
        ...state,
        workers: updateOrganizationWorker(state.workers, event.employee, {
          status: reportFailed ? 'conflict' : 'reported',
          role: event.role,
          reason: reportFailed ? event.outcome : undefined,
        }),
      }
    case 'handoff':
      return {
        ...state,
        organization: state.organization
          ? { ...state.organization, status: 'handoff', handoff: { from: event.from_employee, to: event.to_employee } }
          : state.organization,
      }
    case 'manager_reviewing':
      return {
        ...state,
        organization: state.organization
          ? { ...state.organization, manager: event.manager, status: 'manager_reviewing' }
          : state.organization,
      }
    case 'task_finished':
      if (state.organization && state.organization.taskId !== event.task_id) return state
      return {
        ...state,
        active: false,
        sessionId: event.task_id,
        organization: state.organization
          ? { ...state.organization, status: event.status, reason: event.reason }
          : {
              taskId: event.task_id,
              manager: '中书',
              collaboration: 'recovery',
              status: event.status,
              reason: event.reason,
            },
      }
  }
}

function reduceCodingEvent(state: CodingState, event: CodingUiEvent): CodingState {
  switch (event.kind) {
    case 'plan_created':
      return {
        ...state,
        active: true,
        sessionId: event.session_id,
        risk: event.risk,
        planStepCount: event.step_count,
        steps: [],
        workers: [],
        changes: [],
        verifications: [],
        recoveryMessages: [],
      }
    case 'plan_step_started':
      return {
        ...state,
        active: true,
        sessionId: event.session_id,
        steps: upsertStep(state.steps, {
          id: event.step_id,
          title: event.title,
          status: 'running',
        }),
      }
    case 'plan_step_completed':
      return {
        ...state,
        active: true,
        sessionId: event.session_id,
        steps: upsertStep(state.steps, {
          id: event.step_id,
          title: stepTitle(state.steps, event.step_id),
          status: event.status,
        }),
      }
    case 'worker_started':
      return {
        ...state,
        active: true,
        workers: upsertWorker(state.workers, {
          worker: event.worker,
          taskId: event.task_id,
          status: 'running',
          ownedFiles: event.owned_files,
        }),
      }
    case 'worker_completed':
      return {
        ...state,
        active: true,
        workers: updateWorker(state.workers, event.task_id, {
          status: event.status === 'submitted'
            ? 'submitted'
            : event.success
              ? 'completed'
              : 'conflict',
        }),
      }
    case 'worker_conflict':
      return {
        ...state,
        active: true,
        workers: updateWorker(state.workers, event.task_id, {
          status: 'conflict',
          reason: event.reason,
        }),
      }
    case 'patch_preview':
      return {
        ...state,
        active: true,
        changes: upsertChange(state.changes, {
          path: event.path,
          operation: event.operation,
          summary: event.diff?.summary || event.diff_summary,
          diff: event.diff ?? undefined,
          status: 'preview',
        }),
      }
    case 'patch_applied':
      const existingChange = state.changes.find((change) => change.path === event.path)
      return {
        ...state,
        active: true,
        changes: upsertChange(state.changes, {
          path: event.path,
          operation: event.operation,
          summary: existingChange?.summary ?? (event.changed ? 'changed' : 'unchanged'),
          diff: existingChange?.diff,
          status: 'applied',
          changed: event.changed,
        }),
      }
    case 'verification':
      return {
        ...state,
        active: true,
        verifications: [
          ...state.verifications,
          {
            command: event.command,
            success: event.success,
            exitCode: event.exit_code,
          },
        ],
      }
    case 'recovery_feedback':
      return {
        ...state,
        active: true,
        recoveryMessages: [...state.recoveryMessages, `${event.rule_id}: ${event.message}`],
      }
    case 'context_pressure':
      return {
        ...state,
        active: true,
        contextPressure: event.pressure_percent,
        droppedEvidence: event.dropped_evidence,
        droppedRecent: event.dropped_recent,
      }
    case 'replay_available':
      return {
        ...state,
        active: true,
        replay: {
          conversationId: event.conversation_id,
          replayExecutionId: event.replay_execution_id,
        },
      }
    case 'context_included':
      return {
        ...state,
        active: true,
        contextIncluded: [
          ...state.contextIncluded.slice(-19),
          {
            description: event.description,
            estimatedTokens: event.estimated_tokens,
          },
        ],
      }
  }
}

function upsertStep(steps: PlanStep[], next: PlanStep): PlanStep[] {
  const index = steps.findIndex((step) => step.id === next.id)
  if (index < 0) return [...steps, next]
  return steps.map((step, itemIndex) => (itemIndex === index ? { ...step, ...next } : step))
}

function stepTitle(steps: PlanStep[], stepId: string): string {
  return steps.find((step) => step.id === stepId)?.title ?? stepId
}

function upsertWorker(workers: WorkerState[], next: WorkerState): WorkerState[] {
  const index = workers.findIndex((worker) => worker.taskId === next.taskId || worker.worker === next.worker)
  if (index < 0) return [...workers, next]
  return workers.map((worker, itemIndex) => (itemIndex === index ? { ...worker, ...next } : worker))
}

function updateOrganizationWorker(
  workers: WorkerState[],
  employee: string,
  patch: Partial<WorkerState>,
): WorkerState[] {
  return workers.map((worker) => (worker.worker === employee ? { ...worker, ...patch } : worker))
}

function updateWorker(
  workers: WorkerState[],
  taskId: string,
  patch: Partial<WorkerState>,
): WorkerState[] {
  return workers.map((worker) => (worker.taskId === taskId ? { ...worker, ...patch } : worker))
}

function upsertChange(changes: ChangeState[], next: ChangeState): ChangeState[] {
  const index = changes.findIndex((change) => change.path === next.path)
  if (index < 0) return [...changes, next]
  return changes.map((change, itemIndex) => (itemIndex === index ? { ...change, ...next } : change))
}
