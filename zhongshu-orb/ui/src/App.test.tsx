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
