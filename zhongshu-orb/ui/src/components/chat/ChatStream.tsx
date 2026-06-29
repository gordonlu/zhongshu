import type { ChatState } from '../../state/chatReducer'
import { ToolCallGroup } from './ToolCallGroup'

const maxRenderedMessages = 220

export function ChatStream({
  state,
  onLoadMore,
  onPrompt,
  suggestions = [],
}: {
  state: ChatState
  onLoadMore?: () => void
  onPrompt?: (prompt: string) => void
  suggestions?: string[]
}) {
  const empty = state.messages.length === 0 && !state.streamingAssistant
  const hiddenCount = Math.max(0, state.messages.length - maxRenderedMessages)
  const visibleMessages = hiddenCount > 0 ? state.messages.slice(hiddenCount) : state.messages

  return (
    <div className="chat-stream">
      {onLoadMore ? (
        <div className="history-actions">
          <button type="button" className="secondary-button" onClick={onLoadMore}>
            Load earlier
          </button>
        </div>
      ) : null}
      {empty ? (
        <div className="empty-state">
          <div className="empty-mark">中书</div>
          <h1>Ready for the next task.</h1>
          <p>Keep the request short. The workbench opens when there is plan, review, or verification state to inspect.</p>
          {onPrompt && suggestions.length > 0 ? (
            <div className="empty-suggestions" aria-label="Prompt suggestions">
              {suggestions.map((suggestion) => (
                <button key={suggestion} type="button" onClick={() => onPrompt(suggestion)}>
                  {suggestion}
                </button>
              ))}
            </div>
          ) : null}
        </div>
      ) : null}
      {hiddenCount > 0 ? (
        <div className="history-window-note">
          {hiddenCount} earlier messages are kept in session history but hidden from this render window.
        </div>
      ) : null}
      {visibleMessages.map((message) => (
        <article key={message.id} className={`message ${message.role}`}>
          <div className="message-role">{roleLabel(message.role)}</div>
          <div className="message-content">{message.content}</div>
          <ToolCallGroup entries={message.toolCalls} />
        </article>
      ))}
      <ToolCallGroup activities={state.toolActivities} />
      {state.streamingAssistant ? (
        <article className="message assistant streaming">
          <div className="message-role">Zhongshu</div>
          <div className="message-content">{state.streamingAssistant}</div>
        </article>
      ) : null}
    </div>
  )
}

function roleLabel(role: 'user' | 'assistant' | 'system'): string {
  if (role === 'user') return 'You'
  if (role === 'assistant') return 'Zhongshu'
  return 'System'
}
