import { useCallback, useEffect, useRef, useState } from 'react'
import { ChevronDown, ChevronRight, Copy, Download } from 'lucide-react'
import type { ChatMessage, ChatState } from '../../state/chatReducer'
import { ToolCallGroup } from './ToolCallGroup'
import { renderMarkdown } from './renderMarkdown'

const maxRenderedMessages = 220
const interruptMarker = '\n\n*--- 消息已中断 ---*'
const scrollThreshold = 60
const foldThreshold = 2000

function MessageBody({ message }: { message: ChatMessage }) {
  const [folded, setFolded] = useState(message.content.length > foldThreshold)
  const isLong = message.content.length > foldThreshold

  if (!message.content && !message.interrupted) return null

  const body = message.interrupted
    ? message.content + interruptMarker
    : message.content

  return (
    <div className="message-content md-content">
      {isLong && folded ? (
        <>
          <div className="md-content-truncated">
            {renderMarkdown(body.slice(0, foldThreshold))}
          </div>
          <button type="button" className="message-fold-toggle" onClick={() => setFolded(false)}>
            <ChevronRight size={14} /> Show full message ({message.content.length} chars)
          </button>
        </>
      ) : (
        <>
          {renderMarkdown(body)}
          {isLong ? (
            <button type="button" className="message-fold-toggle" onClick={() => setFolded(true)}>
              <ChevronDown size={14} /> Collapse
            </button>
          ) : null}
        </>
      )}
    </div>
  )
}

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
  const bottomRef = useRef<HTMLDivElement>(null)
  const streamRef = useRef<HTMLDivElement>(null)
  const [userScrolledUp, setUserScrolledUp] = useState(false)
  const [copiedId, setCopiedId] = useState<string | null>(null)

  const isNearBottom = useCallback(() => {
    const el = streamRef.current
    if (!el) return true
    return el.scrollHeight - el.scrollTop - el.clientHeight < scrollThreshold
  }, [])

  const scrollToBottom = useCallback((force = false) => {
    const el = streamRef.current
    if (!el) return
    if (!force && !isNearBottom()) return
    el.scrollTop = el.scrollHeight
  }, [isNearBottom])

  useEffect(() => {
    if (state.streamingAssistant || state.toolActivities.length > 0) {
      scrollToBottom(false)
    }
  }, [state.streamingAssistant, state.toolActivities, scrollToBottom])

  useEffect(() => {
    scrollToBottom(true)
  }, [state.messages.length, scrollToBottom])

  useEffect(() => {
    const el = streamRef.current
    if (!el) return
    const onScroll = () => {
      setUserScrolledUp(!isNearBottom())
    }
    el.addEventListener('scroll', onScroll, { passive: true })
    return () => el.removeEventListener('scroll', onScroll)
  }, [isNearBottom])

  const copyContent = async (content: string, id: string) => {
    try {
      await navigator.clipboard.writeText(content)
      setCopiedId(id)
      setTimeout(() => setCopiedId(null), 2000)
    } catch {
      /* silent */
    }
  }

  const exportMessage = (content: string, id: string) => {
    const ext = 'md'
    const mime = 'text/markdown'
    const blob = new Blob([content], { type: mime })
    const url = URL.createObjectURL(blob)
    const a = document.createElement('a')
    a.href = url
    a.download = `zhongshu-${id}.${ext}`
    a.click()
    URL.revokeObjectURL(url)
  }

  const empty = state.messages.length === 0 && !state.streamingAssistant
  const hiddenCount = Math.max(0, state.messages.length - maxRenderedMessages)
  const visibleMessages = hiddenCount > 0 ? state.messages.slice(hiddenCount) : state.messages

  return (
    <div className="chat-stream" ref={streamRef}>
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
          {message.role === 'assistant' ? <ToolCallGroup entries={message.toolCalls} /> : null}
          <MessageBody message={message} />
          {message.role !== 'assistant' ? <ToolCallGroup entries={message.toolCalls} /> : null}
          {message.content ? (
            <div className="message-actions">
              <button
                type="button"
                className="message-action-button"
                aria-label="Copy message"
                onClick={() => copyContent(message.content, message.id)}
              >
                <Copy size={12} />
                {copiedId === message.id ? 'copied' : 'copy'}
              </button>
              <button
                type="button"
                className="message-action-button"
                aria-label="Export as Markdown"
                onClick={() => exportMessage(message.content, message.id)}
              >
                <Download size={12} />
              </button>
            </div>
          ) : null}
        </article>
      ))}
      {state.streamingAssistant ? (
        <article className="message assistant streaming">
          <div className="message-role">Zhongshu</div>
          <ToolCallGroup activities={state.toolActivities} />
          <div className="message-content md-content">
            {renderMarkdown(state.streamingAssistant, true)}
          </div>
        </article>
      ) : state.toolActivities.length > 0 ? (
        <article className="message assistant streaming">
          <div className="message-role">Zhongshu</div>
          <ToolCallGroup activities={state.toolActivities} />
        </article>
      ) : null}
      <div ref={bottomRef} />
      {userScrolledUp ? (
        <button
          type="button"
          className="scroll-bottom-button"
          onClick={() => { setUserScrolledUp(false); scrollToBottom(true) }}
        >
          ↓ Back to bottom
        </button>
      ) : null}
    </div>
  )
}

function roleLabel(role: 'user' | 'assistant' | 'system'): string {
  if (role === 'user') return 'You'
  if (role === 'assistant') return 'Zhongshu'
  return 'System'
}
