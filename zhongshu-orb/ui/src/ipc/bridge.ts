import type { OverlayToUiEvent } from './events'
import type { UiToOverlayCommand } from './commands'
import { validateCommand } from './commands'

declare global {
  interface Window {
    chrome?: {
      webview?: {
        postMessage: (message: string) => void
      }
    }
    ipc?: {
      postMessage: (message: string) => void
    }
    handleIpc?: (event: OverlayToUiEvent) => void
  }
}

export type IpcBridge = {
  send: (command: UiToOverlayCommand) => void
  install: (handler: (event: OverlayToUiEvent) => void) => () => void
}

export function createIpcBridge(target: Window = window): IpcBridge {
  return {
    send(command) {
      if (!validateCommand(command)) {
        throw new Error(`invalid UI command: ${command.type}`)
      }
      const payload = JSON.stringify(command)
      if (target.chrome?.webview?.postMessage) {
        target.chrome.webview.postMessage(payload)
        return
      }
      if (target.ipc?.postMessage) {
        target.ipc.postMessage(payload)
        return
      }
      if (import.meta.env.DEV) {
        console.debug('[zhongshu-ui] IPC command', command)
      }
    },

    install(handler) {
      target.handleIpc = handler
      return () => {
        if (target.handleIpc === handler) {
          delete target.handleIpc
        }
      }
    },
  }
}
