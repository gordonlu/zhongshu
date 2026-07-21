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
          capabilities: string[]
          responsibility: string
          required: boolean
        }[]
        sequential_handoff: boolean
        max_workers?: number
        target_employee?: string
      }
    }
  | { type: 'list_organization_employees' }
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
      return command.task.objective.trim().length > 0
        && command.task.requirements.length > 0
        && command.task.requirements.length <= 3
        && command.task.requirements.every((requirement) => (
          requirement.role.trim().length > 0
          && requirement.responsibility.trim().length > 0
          && requirement.capabilities.every((capability) => capability.trim().length > 0)
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
    case 'save_settings':
      return typeof command.config === 'object' && command.config !== null
    default:
      return true
  }
}
