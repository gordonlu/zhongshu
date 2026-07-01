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
  max_context_tokens?: number
  mode?: string
}

export type CodingUiEvent =
  | { kind: 'plan_created'; session_id: string; step_count: number; risk: string }
  | { kind: 'plan_step_started'; session_id: string; step_id: string; title: string }
  | { kind: 'plan_step_completed'; session_id: string; step_id: string; status: string }
  | { kind: 'worker_started'; session_id?: string; worker: string; task_id: string; owned_files: string[] }
  | { kind: 'worker_completed'; session_id?: string; worker: string; task_id: string; success: boolean }
  | { kind: 'worker_conflict'; session_id?: string; worker: string; task_id: string; reason: string }
  | { kind: 'patch_preview'; session_id?: string; path: string; operation: string; diff_summary: string; diff?: PatchDiffPayload | null }
  | { kind: 'patch_applied'; session_id?: string; path: string; operation: string; changed: boolean }
  | { kind: 'verification'; command: string; success: boolean; exit_code?: number }
  | { kind: 'recovery_feedback'; rule_id: string; message: string }
  | { kind: 'context_pressure'; pressure_percent: number; dropped_evidence: number; dropped_recent: number }
  | { kind: 'context_included'; description: string; estimated_tokens: number }
  | { kind: 'replay_available'; conversation_id?: number; replay_execution_id?: string }

export type OverlayToUiEvent =
  | { type: 'user_message'; content: string }
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
  | { type: 'verification'; command: string; success: boolean; exit_code?: number; step?: string }
  | { type: 'recovery_feedback'; rule_id: string; message: string }
  | { type: 'phase_transition'; from: string; to: string }
  | { type: 'show_personality' }
  | { type: 'clear' }
