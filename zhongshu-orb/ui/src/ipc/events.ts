export type EntryRole = 'User' | 'Assistant' | 'System'

export type ToolStatus =
  | 'Running'
  | { Done: { success: boolean } }

export type ToolCallEntry = {
  name: string
  status: ToolStatus
}

export type ChatEntry = {
  role: EntryRole
  content: string
  tool_calls: ToolCallEntry[]
}

export type PatchDiffPayload = {
  summary: string
  unified_diff: string
  changed: boolean
  replace_all: boolean
  removed_lines: number
  added_lines: number
  before_hash: string
  after_hash: string
}

export type AuthRequest = {
  request_id: string
  source: string
  tool: string
  command: string
}

export type SettingsConfig = {
  api_key: string
  api_key_saved: boolean
  api_base: string
  model: string
  personality: string
  proxy_port?: string
  bg_enabled?: boolean
  bg_interval?: string
  bg_prompt?: string
  auto_evolve?: boolean
  auto_multi_agent?: boolean
  max_context_tokens?: number
  mode?: string
}

export type CodingUiEvent =
  | { kind: 'plan_created'; session_id: string; step_count: number; risk: string }
  | { kind: 'plan_step_started'; session_id: string; step_id: string; title: string }
  | { kind: 'plan_step_completed'; session_id: string; step_id: string; status: string }
  | { kind: 'worker_started'; session_id?: string; worker: string; task_id: string; owned_files: string[] }
  | { kind: 'worker_completed'; session_id?: string; worker: string; task_id: string; success: boolean; status: string }
  | { kind: 'worker_conflict'; session_id?: string; worker: string; task_id: string; reason: string }
  | { kind: 'patch_preview'; session_id?: string; path: string; operation: string; diff_summary: string; diff?: PatchDiffPayload | null }
  | { kind: 'patch_applied'; session_id?: string; path: string; operation: string; changed: boolean }
  | { kind: 'verification'; command: string; success: boolean; exit_code?: number }
  | { kind: 'recovery_feedback'; rule_id: string; message: string }
  | { kind: 'context_pressure'; pressure_percent: number; dropped_evidence: number; dropped_recent: number }
  | { kind: 'context_included'; description: string; estimated_tokens: number }
  | { kind: 'replay_available'; conversation_id?: number; replay_execution_id?: string }

export type OrganizationUiEvent =
  | { kind: 'routing_decided'; routing_id: string; strategy: string; reason: string; worker_count: number }
  | { kind: 'task_started'; task_id: string; manager: string; collaboration: string }
  | { kind: 'employee_assigned'; task_id: string; employee: string; role: string; responsibility: string; reports_to: string }
  | { kind: 'employee_working'; task_id: string; employee: string; role: string }
  | { kind: 'employee_reported'; task_id: string; employee: string; role: string; outcome: string; success: boolean }
  | { kind: 'handoff'; task_id: string; from_employee: string; to_employee: string }
  | { kind: 'manager_reviewing'; task_id: string; manager: string }
  | { kind: 'task_finished'; task_id: string; status: string; reason?: string }

export type OrganizationEmployeeInfo = {
  name: string
  role: string
  capabilities: string[]
  focus: string
  read_only_eligible: boolean
  blocked_by?: string
  sandbox_eligible?: boolean
  sandbox_blocked_by?: string
}

export type ExecutionNodeState = 'pending' | 'running' | 'succeeded' | 'failed' | 'skipped' | 'cancelled' | 'recovery_required'

export type ExecutionGraphNode = {
  id: string
  kind: string
  objective: string
  executor?: string
  requirements: { capabilities: string[]; read_only: boolean }
  state: ExecutionNodeState
}

export type ExecutionGraphEdge = {
  from: string
  to: string
  kind: string
}

export type ExecutionGraphArtifact = {
  id: string
  producer_node: string
  kind: string
  summary: string
  evidence_refs: string[]
  uncertainties: string[]
}

export type ExecutionGraphReconciliation = {
  node_id: string
  decision: 'confirmed_succeeded' | 'confirmed_failed'
  reason: string
  evidence_refs: string[]
  transition_sequence: number
}

export type ExecutionEffectIntent = {
  id: string
  node_id: string
  expectation: { kind: string; [key: string]: string | number | boolean }
}

export type OrganizationGraphView = {
  store_version: number
  graph: {
    task_id: string
    nodes: ExecutionGraphNode[]
    edges: ExecutionGraphEdge[]
    artifacts: ExecutionGraphArtifact[]
    transitions: unknown[]
    reconciliations: ExecutionGraphReconciliation[]
    effect_intents: ExecutionEffectIntent[]
  }
}

export type OrganizationRecoveryResult = {
  task_id: string
  node_id: string
  action: 'reconcile' | 'abandon'
  assessment: 'confirmed_succeeded' | 'confirmed_failed' | 'inconclusive'
  reason: string
  evidence_refs: string[]
  executed_cleanup_nodes: string[]
  graph: OrganizationGraphView
}

export type OverlayToUiEvent =
  | { type: 'user_message'; content: string }
  | { type: 'stop' }
  | { type: 'delta'; content: string }
  | { type: 'complete' }
  | { type: 'history'; entries: ChatEntry[]; has_more: boolean }
  | { type: 'prepend_history'; entries: ChatEntry[]; has_more: boolean }
  | { type: 'tool_call'; name: string }
  | { type: 'tool_result'; name: string; success: boolean }
  | { type: 'auth'; request: AuthRequest }
  | { type: 'settings'; config: SettingsConfig }
  | { type: 'tasks'; tasks: unknown[] }
  | { type: 'runbooks'; runbooks: unknown[] }
  | { type: 'equipment'; items: unknown[] }
  | { type: 'toast'; text: string }
  | { type: 'state_change'; state: string }
  | { type: 'mode_change'; mode: string }
  | { type: 'zoom'; active: boolean }
  | { type: 'coding'; event: CodingUiEvent }
  | { type: 'organization'; event: OrganizationUiEvent }
  | { type: 'organization_roster'; employees: OrganizationEmployeeInfo[]; max_workers: number }
  | { type: 'organization_graphs'; graphs: OrganizationGraphView[] }
  | { type: 'organization_recovery'; result: OrganizationRecoveryResult }
  | { type: 'verification'; command: string; success: boolean; exit_code?: number; step?: string }
  | { type: 'recovery_feedback'; rule_id: string; message: string }
  | { type: 'phase_transition'; from: string; to: string }
  | { type: 'show_personality' }
  | { type: 'clear' }
