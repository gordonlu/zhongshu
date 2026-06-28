import type { ChatState } from '../../state/chatReducer'

export function ChatStream({
  state,
  onLoadMore,
}: {
  state: ChatState
  onLoadMore?: () => void
}) {
  const empty = state.messages.length === 0 && !state.streamingAssistant

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
      {state.messages.map((message) => (
        <article key={message.id} className={`message ${message.role}`}>
          <div className="message-role">{roleLabel(message.role)}</div>
          <div className="message-content">{message.content}</div>
        </article>
      ))}
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
