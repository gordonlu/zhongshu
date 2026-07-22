import type { SettingsConfig } from './events'

export type UiToOverlayCommand =
  | { type: 'submit'; text: string }
  | { type: 'delegate_review'; text: string }
  | {
      type: 'delegate_organization'
      task: {
        objective: string
        requirements: {
          role: string
          employee?: string
          capabilities: string[]
          responsibility: string
          required: boolean
        }[]
        sequential_handoff: boolean
        max_workers?: number
        target_employee?: string
        mutation?: boolean
        workspace_mode?: 'proposal_only' | 'isolated_sandbox'
        file_scopes?: {
          employee: string
          owned_files: string[]
        }[]
      }
    }
  | { type: 'list_organization_employees' }
  | { type: 'list_organization_graphs' }
  | { type: 'reconcile_organization'; task_id: string; node_id: string }
  | { type: 'abandon_organization_recovery'; task_id: string; node_id: string; reason: string }
  | { type: 'stop' }
  | { type: 'new_conversation' }
  | { type: 'approve'; request_id: string }
  | { type: 'deny'; request_id: string }
  | { type: 'pick_personality'; personality: string }
  | { type: 'save_settings'; config: Partial<SettingsConfig> }
  | { type: 'open_settings' }
  | { type: 'delete_history' }
  | { type: 'load_more' }
  | { type: 'list_tasks' }
  | { type: 'list_runbooks' }
  | { type: 'list_equipment' }
  | { type: 'toggle_equipment'; id: string }
  | { type: 'toggle_zoom' }
  | { type: 'start_drag' }
  | { type: 'minimize' }
  | { type: 'maximize_restore' }
  | { type: 'close_window' }
  | { type: 'cancel_task'; task_id: string }
  | { type: 'complete_task'; task_id: string }

export function validateCommand(command: UiToOverlayCommand): boolean {
  switch (command.type) {
    case 'submit':
    case 'delegate_review':
      return command.text.trim().length > 0
    case 'delegate_organization':
      if (command.task.objective.trim().length === 0
        || command.task.requirements.length === 0
        || command.task.requirements.length > 3
        || !command.task.requirements.every((requirement) => (
          requirement.role.trim().length > 0
          && (requirement.employee === undefined || requirement.employee.trim().length > 0)
          && requirement.responsibility.trim().length > 0
          && requirement.capabilities.every((capability) => capability.trim().length > 0)
        ))) return false
      if (!command.task.mutation) return (command.task.file_scopes?.length ?? 0) === 0
      const employees = command.task.requirements.map((requirement) => requirement.employee)
      const scopes = command.task.file_scopes ?? []
      if (employees.some((employee) => employee === undefined) || scopes.length !== employees.length) return false
      const employeeNames = new Set(employees)
      const scopedEmployeeNames = new Set(scopes.map((scope) => scope.employee))
      return employeeNames.size === employees.length
        && scopedEmployeeNames.size === scopes.length
        && scopedEmployeeNames.size === employeeNames.size
        && scopes.every((scope) => (
          employeeNames.has(scope.employee)
          && scope.owned_files.length > 0
          && scope.owned_files.every(isRelativeOwnedFile)
        ))
    case 'approve':
    case 'deny':
      return command.request_id.length > 0
    case 'pick_personality':
      return command.personality.length > 0
    case 'toggle_equipment':
      return command.id.length > 0
    case 'cancel_task':
    case 'complete_task':
      return command.task_id.length > 0
    case 'reconcile_organization':
      return command.task_id.trim().length > 0 && command.node_id.trim().length > 0
    case 'abandon_organization_recovery':
      return command.task_id.trim().length > 0
        && command.node_id.trim().length > 0
        && command.reason.trim().length > 0
    case 'save_settings':
      return typeof command.config === 'object' && command.config !== null
    default:
      return true
  }
}

function isRelativeOwnedFile(file: string): boolean {
  const trimmed = file.trim()
  if (trimmed.length === 0 || /^(?:[a-zA-Z]:[\\/]|[\\/])/.test(trimmed)) return false
  return !trimmed.split(/[\\/]+/).includes('..')
}
