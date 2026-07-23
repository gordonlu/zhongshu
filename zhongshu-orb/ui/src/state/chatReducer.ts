import type { ChatEntry, ToolCallEntry, OverlayToUiEvent } from '../ipc/events'

export type ChatMessage = {
  id: string
  role: 'user' | 'assistant' | 'system'
  content: string
  toolCalls: ToolCallEntry[]
  model?: string
  duration_ms?: number
  interrupted?: true
}

export type ToolActivity = {
  id: string
  name: string
  status: 'running' | 'done'
  success?: boolean
}

export type ChatState = {
  messages: ChatMessage[]
  streamingAssistant: string
  currentModel?: string
  toolActivities: ToolActivity[]
  hasMoreHistory: boolean
  runtimeState: string
  toast?: string
}

export const initialChatState: ChatState = {
  messages: [],
  streamingAssistant: '',
  toolActivities: [],
  hasMoreHistory: false,
  runtimeState: 'idle',
}

export function chatReducer(state: ChatState, event: OverlayToUiEvent): ChatState {
  switch (event.type) {
    case 'user_message':
      return {
        ...state,
        messages: [
          ...state.messages,
          {
            id: nextMessageId('user'),
            role: 'user',
            content: event.content,
            toolCalls: [],
          },
        ],
        streamingAssistant: '',
      }
    case 'model':
      return {
        ...state,
        currentModel: event.label,
      }
    case 'stop': {
      if (!state.streamingAssistant.trim()) return { ...state, streamingAssistant: '' }
      const partialContent = state.streamingAssistant.trimEnd()
      return {
        ...state,
        messages: [
          ...state.messages,
          {
            id: nextMessageId('assistant'),
            role: 'assistant',
            content: partialContent,
            toolCalls: state.toolActivities.map(toolActivityToEntry),
            model: state.currentModel,
            interrupted: true,
          },
        ],
        streamingAssistant: '',
        currentModel: undefined,
        toolActivities: [],
      }
    }
    case 'delta':
      return {
        ...state,
        streamingAssistant: state.streamingAssistant + event.content,
      }
    case 'complete': {
      if (!state.streamingAssistant.trim() && state.toolActivities.length === 0) return state
      return {
        ...state,
        messages: [
          ...state.messages,
          {
            id: nextMessageId('assistant'),
            role: 'assistant',
            content: state.streamingAssistant,
            toolCalls: state.toolActivities.map(toolActivityToEntry),
            model: state.currentModel,
            duration_ms: event.duration_ms,
          },
        ],
        streamingAssistant: '',
        currentModel: undefined,
        toolActivities: [],
      }
    }
    case 'history':
      return {
        ...state,
        messages: event.entries.map(entryToMessage),
        hasMoreHistory: event.has_more,
        streamingAssistant: '',
      }
    case 'prepend_history':
      return {
        ...state,
        messages: [...event.entries.map(entryToMessage), ...state.messages],
        hasMoreHistory: event.has_more,
      }
    case 'clear':
      return initialChatState
    case 'tool_call':
      return {
        ...state,
        toolActivities: [
          ...state.toolActivities.slice(-80),
          {
            id: nextMessageId('tool'),
            name: event.name,
            status: 'running',
          },
        ],
      }
    case 'tool_result': {
      const index = findLastRunningTool(state.toolActivities, event.name)
      const next = index < 0
        ? [
            ...state.toolActivities.slice(-80),
            {
              id: nextMessageId('tool'),
              name: event.name,
              status: 'done' as const,
              success: event.success,
            },
          ]
        : state.toolActivities.map((tool, itemIndex) => (
            itemIndex === index
              ? { ...tool, status: 'done' as const, success: event.success }
              : tool
          ))
      return { ...state, toolActivities: next }
    }
    case 'state_change':
      return { ...state, runtimeState: event.state }
    case 'toast':
      return { ...state, toast: event.text }
    default:
      return state
  }
}

function toolActivityToEntry(activity: ToolActivity): ToolCallEntry {
  return {
    name: activity.name,
    status: activity.status === 'done'
      ? { Done: { success: activity.success ?? false } }
      : 'Running',
  }
}

function entryToMessage(entry: ChatEntry): ChatMessage {
  return {
    id: nextMessageId(entry.role.toLowerCase()),
    role: entry.role === 'User' ? 'user' : entry.role === 'Assistant' ? 'assistant' : 'system',
    content: entry.content,
    toolCalls: entry.tool_calls,
    model: entry.model,
    duration_ms: entry.duration_ms,
  }
}

function findLastRunningTool(tools: ToolActivity[], name: string): number {
  for (let index = tools.length - 1; index >= 0; index -= 1) {
    const tool = tools[index]
    if (tool.name === name && tool.status === 'running') return index
  }
  return -1
}

let messageId = 0

function nextMessageId(prefix: string): string {
  messageId += 1
  return `${prefix}-${messageId}`
}
