import type { CodingUiEvent, OverlayToUiEvent } from '../ipc/events'

export type PlanStep = {
  id: string
  title: string
  status: string
}

export type WorkerState = {
  worker: string
  taskId: string
  status: 'running' | 'completed' | 'conflict'
  ownedFiles: string[]
  reason?: string
}

export type ChangeState = {
  path: string
  operation: string
  summary: string
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
  contextPressure?: number
  replay?: {
    conversationId?: number
    replayExecutionId?: string
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
}

export function codingReducer(state: CodingState, event: OverlayToUiEvent): CodingState {
  if (event.type !== 'coding') return state
  return reduceCodingEvent(state, event.event)
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
          status: event.success ? 'completed' : 'conflict',
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
          summary: event.diff_summary,
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
      return state
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
  const index = workers.findIndex((worker) => worker.taskId === next.taskId)
  if (index < 0) return [...workers, next]
  return workers.map((worker, itemIndex) => (itemIndex === index ? { ...worker, ...next } : worker))
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
