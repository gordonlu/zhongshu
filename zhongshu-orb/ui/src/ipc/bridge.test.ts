import { describe, expect, it, vi } from 'vitest'
import { createIpcBridge } from './bridge'
import type { OverlayToUiEvent } from './events'
import { chatReducer, initialChatState } from '../state/chatReducer'
import { codingReducer, initialCodingState } from '../state/codingReducer'

function fakeWindow(overrides: Partial<Window> = {}): Window {
  return overrides as Window
}

describe('createIpcBridge', () => {
  it('sends commands through the WebView2 host when available', () => {
    const postMessage = vi.fn()
    const bridge = createIpcBridge(fakeWindow({
      chrome: { webview: { postMessage } },
    }))

    bridge.send({ type: 'submit', text: 'inspect harness' })

    expect(postMessage).toHaveBeenCalledWith(JSON.stringify({
      type: 'submit',
      text: 'inspect harness',
    }))
  })

  it('falls back to the generic ipc host', () => {
    const postMessage = vi.fn()
    const bridge = createIpcBridge(fakeWindow({
      ipc: { postMessage },
    }))

    bridge.send({ type: 'toggle_zoom' })

    expect(postMessage).toHaveBeenCalledWith(JSON.stringify({ type: 'toggle_zoom' }))
  })

  it('sends native drag commands through the host bridge', () => {
    const postMessage = vi.fn()
    const bridge = createIpcBridge(fakeWindow({
      chrome: { webview: { postMessage } },
    }))

    bridge.send({ type: 'start_drag' })

    expect(postMessage).toHaveBeenCalledWith(JSON.stringify({ type: 'start_drag' }))
  })

  it('rejects invalid commands before they reach the host', () => {
    const postMessage = vi.fn()
    const bridge = createIpcBridge(fakeWindow({
      chrome: { webview: { postMessage } },
    }))

    expect(() => bridge.send({ type: 'submit', text: '   ' })).toThrow('invalid UI command')
    expect(postMessage).not.toHaveBeenCalled()
  })

  it('installs and removes a Rust event handler', () => {
    const target = fakeWindow()
    const handler = vi.fn()
    const bridge = createIpcBridge(target)

    const uninstall = bridge.install(handler)
    const event: OverlayToUiEvent = { type: 'delta', content: 'hello' }

    target.handleIpc?.(event)

    expect(handler).toHaveBeenCalledWith(event)

    uninstall()

    expect(target.handleIpc).toBeUndefined()
  })

  it('feeds Rust-shaped chat and coding events into reducers', () => {
    const target = fakeWindow()
    const bridge = createIpcBridge(target)
    let chatState = initialChatState
    let codingState = initialCodingState

    bridge.install((event) => {
      chatState = chatReducer(chatState, event)
      codingState = codingReducer(codingState, event)
    })

    const events: OverlayToUiEvent[] = [
      { type: 'history', has_more: false, entries: [{ role: 'User', content: 'fix tests', tool_calls: [] }] },
      { type: 'delta', content: 'working' },
      { type: 'complete' },
      { type: 'coding', event: { kind: 'plan_created', session_id: 's1', step_count: 1, risk: 'low' } },
      { type: 'coding', event: { kind: 'verification', command: 'cargo test', success: true, exit_code: 0 } },
    ]

    for (const event of events) {
      target.handleIpc?.(event)
    }

    expect(chatState.messages.map((message) => message.content)).toEqual(['fix tests', 'working'])
    expect(codingState.sessionId).toBe('s1')
    expect(codingState.verifications[0]).toMatchObject({ command: 'cargo test', success: true })
  })
})
