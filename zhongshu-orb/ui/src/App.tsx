import { type MouseEvent, useEffect, useMemo, useReducer, useState } from 'react'
import {
  BookOpen,
  CheckCircle2,
  CircleStop,
  ClipboardList,
  Code2,
  PanelRight,
  Send,
  Settings,
  ShieldAlert,
  Wrench,
  ZoomIn,
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

const bridge = createIpcBridge()

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
  const [toast, setToast] = useState<string | null>(null)
  const [composerText, setComposerText] = useState('')

  useEffect(() => {
    return bridge.install((event: OverlayToUiEvent) => {
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
      } else if (event.type === 'clear') {
        setAuthRequest(null)
        setToast(null)
      }
    })
  }, [])

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
    <div className="app-shell">
      <header className="titlebar" onMouseDown={startWindowDrag}>
        <div className="brand">
          <div className="brand-mark">Z</div>
          <div>
            <div className="brand-title">Zhongshu</div>
            <div className="brand-subtitle">Desktop assistant</div>
          </div>
        </div>
        <div className="titlebar-status" data-state={statusText}>
          {authRequest ? <ShieldAlert size={14} /> : codingState.active ? <Code2 size={14} /> : <CheckCircle2 size={14} />}
          <span>{statusText}</span>
        </div>
        <div className="titlebar-actions">
          <button
            type="button"
            className="icon-button"
            aria-label="Toggle coding workbench"
            onClick={() => setWorkbenchOpen((value) => !value)}
          >
            <PanelRight size={16} />
          </button>
          <button
            type="button"
            className="icon-button"
            aria-label="List tasks"
            onClick={() => bridge.send({ type: 'list_tasks' })}
          >
            <ClipboardList size={16} />
          </button>
          <button
            type="button"
            className="icon-button"
            aria-label="List runbooks"
            onClick={() => bridge.send({ type: 'list_runbooks' })}
          >
            <BookOpen size={16} />
          </button>
          <button
            type="button"
            className="icon-button"
            aria-label="List equipment"
            onClick={() => bridge.send({ type: 'list_equipment' })}
          >
            <Wrench size={16} />
          </button>
          <button
            type="button"
            className={zoomActive ? 'icon-button active' : 'icon-button'}
            aria-label="Toggle zoom"
            onClick={() => bridge.send({ type: 'toggle_zoom' })}
          >
            <ZoomIn size={16} />
          </button>
          <button
            type="button"
            className="icon-button"
            aria-label="Open settings"
            onClick={() => bridge.send({ type: 'open_settings' })}
          >
            <Settings size={16} />
          </button>
        </div>
      </header>

      {isCodingMode ? (
        <section className="status-strip" aria-label="Coding status">
          <span>Plan {codingState.steps.length}/{codingState.planStepCount || '-'}</span>
          <span>Workers {codingState.workers.length}</span>
          <span>Changes {codingState.changes.length}</span>
          <span>Checks {codingState.verifications.length}</span>
          {codingState.contextPressure !== undefined ? <span>Context {codingState.contextPressure}%</span> : null}
        </section>
      ) : null}

      <main className={isCodingMode && workbenchOpen ? 'main-grid has-workbench' : 'main-grid'}>
        <section className="chat-pane" aria-label="Conversation">
          <ChatStream
            state={chatState}
            onLoadMore={chatState.hasMoreHistory ? () => bridge.send({ type: 'load_more' }) : undefined}
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
          onClick={() => bridge.send({ type: 'stop' })}
        >
          <CircleStop size={16} />
        </button>
        <Composer
          value={composerText}
          placeholder={isCodingMode ? 'Describe the coding task...' : 'Ask Zhongshu...'}
          onChange={setComposerText}
          onSubmit={submitComposer}
        />
        <button
          type="button"
          className="send-button"
          aria-label="Send"
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

      {toast ? <div className="toast">{toast}</div> : null}
    </div>
  )
}
