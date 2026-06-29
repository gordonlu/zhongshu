import type { ChatState } from '../../state/chatReducer'
import { ToolCallGroup } from './ToolCallGroup'

const maxRenderedMessages = 220

export function ChatStream({
  state,
  onLoadMore,
}: {
  state: ChatState
  onLoadMore?: () => void
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
          <h1>Zhongshu</h1>
          <p>Ask from the desktop overlay. Expand the workbench when plan, changes, checks, or replay evidence matters.</p>
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
