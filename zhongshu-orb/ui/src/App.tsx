import { type MouseEvent, useEffect, useMemo, useReducer, useRef, useState } from 'react'
import {
  BookOpen,
  CheckCircle2,
  CircleStop,
  ClipboardList,
  Code2,
  Minus,
  Moon,
  PanelRight,
  Send,
  Settings,
  ShieldAlert,
  Square,
  Sun,
  Wrench,
  X,
  ZoomIn,
  ZoomOut,
} from 'lucide-react'
import { createIpcBridge } from './ipc/bridge'
import type { AuthRequest, OverlayToUiEvent, SettingsConfig } from './ipc/events'
import { chatReducer, initialChatState } from './state/chatReducer'
import { codingReducer, initialCodingState } from './state/codingReducer'
import { ChatStream } from './components/chat/ChatStream'
import { Composer } from './components/chat/Composer'
import { CodingWorkbench } from './components/coding/CodingWorkbench'
import { ApprovalBar } from './components/ApprovalBar'
import { SettingsDialog } from './components/settings/SettingsDialog'
import { ResourceDialog } from './components/resources/ResourceDialog'
import { demoCodingEvents } from './dev/fixtures'

const bridge = createIpcBridge()
const iconSize = 16

const assistantPrompts = [
  'Review current changes',
  'Plan the next coding step',
  'Explain the failing check',
]

type Theme = 'dark' | 'light'

function initialTheme(): Theme {
  const stored = window.localStorage.getItem('zhongshu.theme')
  if (stored === 'dark' || stored === 'light') return stored
  return window.matchMedia?.('(prefers-color-scheme: light)').matches ? 'light' : 'dark'
}

type ResourceDialogState = {
  kind: 'tasks' | 'runbooks' | 'equipment'
  items: unknown[]
}

export function App() {
  const [chatState, dispatchChat] = useReducer(chatReducer, initialChatState)
  const [codingState, dispatchCoding] = useReducer(codingReducer, initialCodingState)
  const [mode, setMode] = useState('assistant')
  const [authRequest, setAuthRequest] = useState<AuthRequest | null>(null)
  const [settingsConfig, setSettingsConfig] = useState<SettingsConfig | null>(null)
  const [resourceDialog, setResourceDialog] = useState<ResourceDialogState | null>(null)
  const [workbenchOpen, setWorkbenchOpen] = useState(true)
  const [zoomActive, setZoomActive] = useState(false)
  const [isMaximized, setIsMaximized] = useState(false)
  const [theme, setTheme] = useState<Theme>(initialTheme)
  const [toast, setToast] = useState<string | null>(null)
  const [showPersonality, setShowPersonality] = useState(false)
  const [composerText, setComposerText] = useState('')
  const demoLoaded = useRef(false)
  const pendingDelta = useRef('')
  const pendingDeltaFrame = useRef<number | null>(null)

  useEffect(() => {
    return bridge.install((event: OverlayToUiEvent) => {
      if (event.type === 'delta') {
        pendingDelta.current += event.content
        if (pendingDeltaFrame.current === null) {
          pendingDeltaFrame.current = window.requestAnimationFrame(() => {
            pendingDeltaFrame.current = null
            if (!pendingDelta.current) return
            dispatchChat({ type: 'delta', content: pendingDelta.current })
            pendingDelta.current = ''
          })
        }
        return
      }
      if (pendingDelta.current) {
        if (pendingDeltaFrame.current !== null) {
          window.cancelAnimationFrame(pendingDeltaFrame.current)
          pendingDeltaFrame.current = null
        }
        dispatchChat({ type: 'delta', content: pendingDelta.current })
        pendingDelta.current = ''
      }
      dispatchChat(event)
      dispatchCoding(event)
      if (event.type === 'mode_change') {
        setMode(event.mode)
      } else if (event.type === 'auth') {
        setAuthRequest(event.request)
      } else if (event.type === 'settings') {
        setSettingsConfig(event.config)
      } else if (event.type === 'tasks') {
        setResourceDialog({ kind: 'tasks', items: event.tasks })
      } else if (event.type === 'runbooks') {
        setResourceDialog({ kind: 'runbooks', items: event.runbooks })
      } else if (event.type === 'equipment') {
        setResourceDialog({ kind: 'equipment', items: event.items })
      } else if (event.type === 'toast') {
        setToast(event.text)
      } else if (event.type === 'zoom') {
        setZoomActive(event.active)
      } else if (event.type === 'show_personality') {
        setShowPersonality(true)
      } else if (event.type === 'clear') {
        setAuthRequest(null)
        setToast(null)
      }
    })
  }, [])

  useEffect(() => {
    if (!import.meta.env.DEV) return
    if (demoLoaded.current) return
    const params = new URLSearchParams(window.location.search)
    if (params.get('demo') !== 'coding') return
    demoLoaded.current = true
    for (const event of demoCodingEvents) {
      dispatchChat(event)
      dispatchCoding(event)
      if (event.type === 'mode_change') {
        setMode(event.mode)
        setWorkbenchOpen(true)
      }
    }
  }, [])

  useEffect(() => {
    window.localStorage.setItem('zhongshu.theme', theme)
  }, [theme])

  useEffect(() => {
    if (!toast) return undefined
    const timer = window.setTimeout(() => setToast(null), 3200)
    return () => window.clearTimeout(timer)
  }, [toast])

  const isCodingMode = mode === 'coding' || codingState.active
  const submitComposer = () => {
    const text = composerText.trim()
    if (!text) return
    bridge.send({ type: 'submit', text })
    setComposerText('')
  }
  const startWindowDrag = (event: MouseEvent<HTMLElement>) => {
    if (event.button !== 0) return
    const target = event.target as HTMLElement | null
    if (target?.closest('button,input,select,textarea,a,[data-no-drag]')) return
    bridge.send({ type: 'start_drag' })
  }
  const statusText = useMemo(() => {
    if (authRequest) return `Approval: ${authRequest.tool}`
    if (codingState.verifications.some((item) => !item.success)) return 'Verification failed'
    if (codingState.active) return 'Coding task running'
    return chatState.runtimeState
  }, [authRequest, chatState.runtimeState, codingState.active, codingState.verifications])

  return (
  <>
    <div className="app-shell" data-theme={theme} data-layout={isCodingMode ? 'coding' : 'assistant'}>
      <header className="titlebar" onMouseDown={startWindowDrag}>
        <div className="brand">
          <div className="brand-mark" aria-hidden="true">中书</div>
          <div>
            <div className="brand-title">中书</div>
            <div className="brand-subtitle">Agent workspace</div>
          </div>
        </div>
        <div className="mode-switch" data-no-drag>
          <button
            type="button"
            data-tooltip-dir="below"
            data-tooltip="Assistant mode"
            className={mode === 'assistant' ? 'active' : undefined}
            onClick={() => {
              setMode('assistant')
              bridge.send({ type: 'save_settings', config: { mode: 'assistant' } })
            }}
          >
            Assistant
          </button>
          <button
            type="button"
            data-tooltip-dir="below"
            data-tooltip="Coding mode"
            className={mode === 'coding' ? 'active' : undefined}
            onClick={() => {
              setMode('coding')
              setWorkbenchOpen(true)
              bridge.send({ type: 'save_settings', config: { mode: 'coding' } })
            }}
          >
            Coding
          </button>
        </div>
        <div className="titlebar-right">
        <div className="titlebar-status" data-state={statusText}>
          {authRequest ? <ShieldAlert size={14} /> : codingState.active ? <Code2 size={14} /> : <CheckCircle2 size={14} />}
          <span>{statusText}</span>
        </div>
        <div className="titlebar-actions">
          {isCodingMode ? (
            <button
              type="button"
              className="icon-button optional-title-action"
              aria-label="Toggle coding workbench"
              data-tooltip-dir="below"
              data-tooltip="Coding workbench"
              onClick={() => setWorkbenchOpen((value) => !value)}
            >
              <PanelRight size={iconSize} />
            </button>
          ) : null}
          <button
            type="button"
            className="icon-button optional-title-action"
            aria-label="List tasks"
            data-tooltip-dir="below"
            data-tooltip="Tasks"
            onClick={() => bridge.send({ type: 'list_tasks' })}
          >
            <ClipboardList size={iconSize} />
          </button>
          <button
            type="button"
            className="icon-button optional-title-action"
            aria-label="List runbooks"
            data-tooltip-dir="below"
            data-tooltip="Runbooks"
            onClick={() => bridge.send({ type: 'list_runbooks' })}
          >
            <BookOpen size={iconSize} />
          </button>
          <button
            type="button"
            className="icon-button optional-title-action"
            aria-label="List equipment"
            data-tooltip-dir="below"
            data-tooltip="Equipment"
            onClick={() => bridge.send({ type: 'list_equipment' })}
          >
            <Wrench size={iconSize} />
          </button>
          <button
            type="button"
            className="icon-button"
            aria-label="Toggle theme"
            data-tooltip-dir="below"
            data-tooltip={theme === 'dark' ? 'Light mode' : 'Dark mode'}
            onClick={() => setTheme((value) => (value === 'dark' ? 'light' : 'dark'))}
          >
            {theme === 'dark' ? <Sun size={iconSize} /> : <Moon size={iconSize} />}
          </button>
          <button
            type="button"
            className={zoomActive ? 'icon-button active' : 'icon-button'}
            aria-label={zoomActive ? 'Zoom out' : 'Zoom in'}
            data-tooltip-dir="below"
            data-tooltip={zoomActive ? 'Zoom out' : 'Zoom in'}
            onClick={() => bridge.send({ type: 'toggle_zoom' })}
          >
            {zoomActive ? <ZoomOut size={iconSize} /> : <ZoomIn size={iconSize} />}
          </button>
          <button
            type="button"
            className="icon-button"
            aria-label="Open settings"
            data-tooltip-dir="below"
            data-tooltip="Settings"
            onClick={() => bridge.send({ type: 'open_settings' })}
          >
            <Settings size={iconSize} />
          </button>
          <span className="titlebar-separator" />
          <button
            type="button"
            className="win-button"
            aria-label="Minimize"
            data-tooltip-dir="below"
            data-tooltip="Minimize"
            onClick={() => bridge.send({ type: 'minimize' })}
          >
            <Minus size={14} />
          </button>
          <button
            type="button"
            className="win-button"
            aria-label={isMaximized ? 'Restore' : 'Maximize'}
            data-tooltip-dir="below"
            data-tooltip={isMaximized ? 'Restore' : 'Maximize'}
            onClick={() => {
              setIsMaximized((v) => !v)
              bridge.send({ type: 'maximize_restore' })
            }}
          >
            <Square size={12} />
          </button>
          <button
            type="button"
            className="win-button win-close"
            aria-label="Close"
            data-tooltip-dir="below"
            data-tooltip="Close"
            onClick={() => bridge.send({ type: 'close_window' })}
          >
            <X size={14} />
          </button>
        </div>
        </div>
      </header>

      {isCodingMode ? (
        <section className="status-strip" aria-label="Coding status">
          <span>Plan {codingState.steps.length}/{codingState.planStepCount || '-'}</span>
          <span>Agents {codingState.workers.length}</span>
          <span>Review {codingState.changes.length}</span>
          <span>Checks {codingState.verifications.length}</span>
          {codingState.contextPressure !== undefined ? <span>Context {codingState.contextPressure}%</span> : null}
          {codingState.phase ? <span>{codingState.phase.from} to {codingState.phase.to}</span> : null}
        </section>
      ) : null}

      <main className={isCodingMode && workbenchOpen ? 'main-grid has-workbench' : 'main-grid'}>
        <section className="chat-pane" aria-label="Conversation">
          <ChatStream
            state={chatState}
            onLoadMore={chatState.hasMoreHistory ? () => bridge.send({ type: 'load_more' }) : undefined}
            onPrompt={(prompt) => setComposerText(prompt)}
            suggestions={assistantPrompts}
          />
        </section>
        {isCodingMode && workbenchOpen ? (
          <CodingWorkbench state={codingState} />
        ) : null}
      </main>

      {authRequest ? (
        <ApprovalBar
          request={authRequest}
          onApprove={() => {
            bridge.send({ type: 'approve', request_id: authRequest.request_id })
            setAuthRequest(null)
          }}
          onDeny={() => {
            bridge.send({ type: 'deny', request_id: authRequest.request_id })
            setAuthRequest(null)
          }}
        />
      ) : null}

      <footer className="composer-shell">
        <button
          type="button"
          className="stop-button"
          aria-label="Stop"
          data-tooltip="Stop"
          onClick={() => bridge.send({ type: 'stop' })}
        >
          <CircleStop size={16} />
        </button>
        <Composer
          value={composerText}
          placeholder={isCodingMode ? 'Describe the task or review request...' : 'Ask Zhongshu what to do next.'}
          onChange={setComposerText}
          onSubmit={submitComposer}
        />
        <button
          type="button"
          className="send-button"
          aria-label="Send"
          data-tooltip="Send"
          onClick={submitComposer}
        >
          <Send size={16} />
        </button>
      </footer>

      {settingsConfig ? (
        <SettingsDialog
          config={settingsConfig}
          onClose={() => setSettingsConfig(null)}
          onDeleteHistory={() => {
            bridge.send({ type: 'delete_history' })
            setSettingsConfig(null)
          }}
          onSave={(config) => {
            bridge.send({ type: 'save_settings', config })
            setSettingsConfig(null)
          }}
        />
      ) : null}

      {resourceDialog ? (
        <ResourceDialog
          kind={resourceDialog.kind}
          items={resourceDialog.items}
          onClose={() => setResourceDialog(null)}
          onToggleEquipment={(id) => bridge.send({ type: 'toggle_equipment', id })}
          onCancelTask={(task_id) => bridge.send({ type: 'cancel_task', task_id })}
          onCompleteTask={(task_id) => bridge.send({ type: 'complete_task', task_id })}
        />
      ) : null}

      {showPersonality ? (
        <div className="modal-backdrop" role="presentation">
          <section className="modal-panel personality-panel" role="dialog" aria-modal="true" aria-label="Personality">
            <header className="modal-header">
              <h2>Personality</h2>
              <button type="button" className="icon-button" aria-label="Close personality" onClick={() => setShowPersonality(false)}>
                <X size={iconSize} />
              </button>
            </header>
            <div className="personality-grid">
              {['古典', '极客', '温度'].map((personality) => (
                <button
                  key={personality}
                  type="button"
                  className="personality-option"
                  onClick={() => {
                    bridge.send({ type: 'pick_personality', personality })
                    setShowPersonality(false)
                  }}
                >
                  {personality}
                </button>
              ))}
            </div>
          </section>
        </div>
      ) : null}

    </div>
    <div className="window-border" />
    {toast ? <div className="toast">{toast}</div> : null}
  </>)
}
