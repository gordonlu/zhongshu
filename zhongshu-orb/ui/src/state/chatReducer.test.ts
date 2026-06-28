import { describe, expect, it } from 'vitest'
import { chatReducer, initialChatState } from './chatReducer'

describe('chatReducer', () => {
  it('streams and completes assistant text', () => {
    const streamed = chatReducer(initialChatState, { type: 'delta', content: 'hello' })
    const completed = chatReducer(streamed, { type: 'complete' })

    expect(completed.streamingAssistant).toBe('')
    expect(completed.messages).toMatchObject([
      { role: 'assistant', content: 'hello' },
    ])
  })

  it('loads history entries from Rust contract shape', () => {
    const state = chatReducer(initialChatState, {
      type: 'history',
      has_more: true,
      entries: [
        { role: 'User', content: 'task', tool_calls: [] },
        { role: 'Assistant', content: 'done', tool_calls: [] },
      ],
    })

    expect(state.hasMoreHistory).toBe(true)
    expect(state.messages.map((message) => message.role)).toEqual(['user', 'assistant'])
  })
})
