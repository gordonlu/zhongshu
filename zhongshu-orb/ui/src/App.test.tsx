import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import type { SettingsConfig } from './ipc/events'

const settingsConfig: SettingsConfig = {
  api_key: '',
  api_key_saved: true,
  api_base: 'https://example.test',
  model: 'deepseek-v4-flash',
  personality: 'concise',
  mode: 'assistant',
}

describe('App IPC interactions', () => {
  afterEach(() => {
    cleanup()
    vi.restoreAllMocks()
    vi.resetModules()
    delete window.handleIpc
    delete window.chrome
  })

  it('opens settings from Rust and saves through the host bridge', async () => {
    const postMessage = installWebViewHost()
    const { App } = await import('./App')

    render(<App />)

    act(() => {
      window.handleIpc?.({ type: 'settings', config: settingsConfig })
    })

    expect(screen.getByRole('dialog', { name: 'Settings' })).toBeInTheDocument()

    fireEvent.change(screen.getByLabelText('Model'), {
      target: { value: 'deepseek-v4-flash-next' },
    })
    fireEvent.click(screen.getByRole('button', { name: 'Save' }))

    expect(JSON.parse(postMessage.mock.calls.at(-1)?.[0] ?? '{}')).toMatchObject({
      type: 'save_settings',
      config: {
        model: 'deepseek-v4-flash-next',
      },
    })
  })

  it('routes approval and task commands through the host bridge', async () => {
    const postMessage = installWebViewHost()
    const { App } = await import('./App')

    render(<App />)

    act(() => {
      window.handleIpc?.({
        type: 'auth',
        request: {
          request_id: 'req-1',
          source: 'tool',
          tool: 'shell',
          command: 'cargo test',
        },
      })
    })
    fireEvent.click(screen.getByRole('button', { name: 'Allow' }))
    expect(JSON.parse(postMessage.mock.calls.at(-1)?.[0] ?? '{}')).toEqual({
      type: 'approve',
      request_id: 'req-1',
    })

    act(() => {
      window.handleIpc?.({
        type: 'tasks',
        tasks: [{ id: 'task-1', title: 'Verify UI', status: 'running' }],
      })
    })
    fireEvent.click(screen.getByRole('button', { name: 'Complete' }))
    expect(JSON.parse(postMessage.mock.calls.at(-1)?.[0] ?? '{}')).toEqual({
      type: 'complete_task',
      task_id: 'task-1',
    })
  })

  it('starts native window drag from the titlebar but not action buttons', async () => {
    const postMessage = installWebViewHost()
    const { App } = await import('./App')

    const { container } = render(<App />)

    const titlebar = container.querySelector('.titlebar')
    expect(titlebar).not.toBeNull()

    fireEvent.mouseDown(titlebar!, { button: 0 })
    expect(JSON.parse(postMessage.mock.calls.at(-1)?.[0] ?? '{}')).toEqual({
      type: 'start_drag',
    })

    const callCount = postMessage.mock.calls.length
    const listTasksButton = titlebar!.querySelector('[aria-label="List tasks"]')
    expect(listTasksButton).not.toBeNull()
    fireEvent.mouseDown(listTasksButton!, { button: 0 })
    expect(postMessage).toHaveBeenCalledTimes(callCount)
  })

  it('routes mode switch and personality picker commands', async () => {
    const postMessage = installWebViewHost()
    const { App } = await import('./App')

    render(<App />)

    fireEvent.click(screen.getByRole('button', { name: 'Coding' }))
    expect(JSON.parse(postMessage.mock.calls.at(-1)?.[0] ?? '{}')).toEqual({
      type: 'save_settings',
      config: { mode: 'coding' },
    })

    act(() => {
      window.handleIpc?.({ type: 'show_personality' })
    })
    fireEvent.click(screen.getByRole('button', { name: '极客' }))
    expect(JSON.parse(postMessage.mock.calls.at(-1)?.[0] ?? '{}')).toEqual({
      type: 'pick_personality',
      personality: '极客',
    })
  })

  it('keeps Enter composition-safe and restores composer focus', async () => {
    const postMessage = installWebViewHost()
    const { App } = await import('./App')

    render(<App />)

    const composer = screen.getByPlaceholderText('Ask Zhongshu what to do next.')
    expect(composer).toHaveFocus()

    fireEvent.change(composer, { target: { value: '中文输入' } })
    fireEvent.compositionStart(composer)
    fireEvent.keyDown(composer, { key: 'Enter' })
    expect(postMessage).not.toHaveBeenCalled()

    fireEvent.compositionEnd(composer)
    fireEvent.keyDown(composer, { key: 'Enter' })
    expect(JSON.parse(postMessage.mock.calls.at(-1)?.[0] ?? '{}')).toEqual({
      type: 'submit',
      text: '中文输入',
    })
    expect(composer).toHaveFocus()
  })

  it('shows submitted user text immediately and ignores the native echo', async () => {
    const postMessage = installWebViewHost()
    const { App } = await import('./App')

    render(<App />)

    const composer = screen.getByPlaceholderText('Ask Zhongshu what to do next.')
    fireEvent.change(composer, { target: { value: 'show me instantly' } })
    fireEvent.click(screen.getByRole('button', { name: 'Send' }))

    expect(JSON.parse(postMessage.mock.calls.at(-1)?.[0] ?? '{}')).toEqual({
      type: 'submit',
      text: 'show me instantly',
    })
    expect(screen.getByText('show me instantly')).toBeInTheDocument()

    act(() => {
      window.handleIpc?.({ type: 'user_message', content: 'show me instantly' })
    })

    expect(screen.getAllByText('show me instantly')).toHaveLength(1)
  })

  it('delegates a coding review without changing the normal send command', async () => {
    const postMessage = installWebViewHost()
    const { App } = await import('./App')

    render(<App />)

    fireEvent.click(screen.getByRole('button', { name: 'Coding' }))
    const composer = screen.getByPlaceholderText('Describe the task or review request...')
    fireEvent.change(composer, { target: { value: 'review the current changes' } })
    fireEvent.click(screen.getByRole('button', { name: 'Delegate review to two workers' }))

    expect(JSON.parse(postMessage.mock.calls.at(-1)?.[0] ?? '{}')).toEqual({
      type: 'delegate_review',
      text: 'review the current changes',
    })

    fireEvent.change(composer, { target: { value: 'continue normally' } })
    fireEvent.click(screen.getByRole('button', { name: 'Send' }))
    expect(JSON.parse(postMessage.mock.calls.at(-1)?.[0] ?? '{}')).toEqual({
      type: 'submit',
      text: 'continue normally',
    })
  })

  it('closes modal surfaces before hiding the overlay on Escape', async () => {
    const postMessage = installWebViewHost()
    const { App } = await import('./App')

    render(<App />)

    act(() => {
      window.handleIpc?.({ type: 'settings', config: settingsConfig })
    })
    expect(screen.getByRole('dialog', { name: 'Settings' })).toBeInTheDocument()

    fireEvent.keyDown(document, { key: 'Escape' })
    expect(screen.queryByRole('dialog', { name: 'Settings' })).not.toBeInTheDocument()
    expect(postMessage).not.toHaveBeenCalled()
    expect(screen.getByPlaceholderText('Ask Zhongshu what to do next.')).toHaveFocus()

    fireEvent.keyDown(document, { key: 'Escape' })
    expect(JSON.parse(postMessage.mock.calls.at(-1)?.[0] ?? '{}')).toEqual({
      type: 'close_window',
    })
  })

  it('batches streaming deltas into an animation frame and flushes before completion', async () => {
    installWebViewHost()
    let animationFrame: FrameRequestCallback | null = null
    const requestAnimationFrame = vi
      .spyOn(window, 'requestAnimationFrame')
      .mockImplementation((callback: FrameRequestCallback) => {
        animationFrame = callback
        return 7
      })
    const cancelAnimationFrame = vi.spyOn(window, 'cancelAnimationFrame')
    const { App } = await import('./App')

    render(<App />)

    act(() => {
      window.handleIpc?.({ type: 'delta', content: 'hel' })
      window.handleIpc?.({ type: 'delta', content: 'lo' })
    })

    expect(requestAnimationFrame).toHaveBeenCalledTimes(1)
    expect(screen.queryByText('hello')).not.toBeInTheDocument()

    act(() => {
      animationFrame?.(0)
    })

    expect(screen.getByText('hello')).toBeInTheDocument()

    act(() => {
      window.handleIpc?.({ type: 'delta', content: '!' })
      window.handleIpc?.({ type: 'complete' })
    })

    expect(cancelAnimationFrame).toHaveBeenCalledWith(7)
    expect(screen.getByText('hello!')).toBeInTheDocument()
  })
})

function installWebViewHost() {
  const postMessage = vi.fn()
  Object.defineProperty(window, 'chrome', {
    configurable: true,
    value: {
      webview: { postMessage },
    },
  })
  return postMessage
}
