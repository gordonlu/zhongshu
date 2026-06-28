import type { SettingsConfig } from './events'

export type UiToOverlayCommand =
  | { type: 'submit'; text: string }
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
  | { type: 'cancel_task'; task_id: string }
  | { type: 'complete_task'; task_id: string }

export function validateCommand(command: UiToOverlayCommand): boolean {
  switch (command.type) {
    case 'submit':
      return command.text.trim().length > 0
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
