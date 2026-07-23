import { useState } from 'react'
import {
  Activity,
  BookOpen,
  Bot,
  Bug,
  Clock,
  History,
  Monitor,
  Server,
  Wrench,
  X,
} from 'lucide-react'
import type { AuthEntry, CompressEntry, MemoryEntry, ChromeState, DebugEntry } from '../../ipc/events'
import { AuthHistoryPanel } from './AuthHistoryPanel'
import { ChromeStatusCard } from './ChromeStatusCard'
import { CompressionView } from './CompressionView'
import { DebugPanel } from './DebugPanel'
import { MemoryPanel } from './MemoryPanel'

type PanelId = 'tasks' | 'runbooks' | 'equipment' | 'auth' | 'memory' | 'chrome' | 'debug'

type TabDef = {
  id: PanelId
  label: string
  icon: typeof Activity
  available?: true
}

const tabs: TabDef[] = [
  { id: 'tasks', label: 'Tasks', icon: Activity, available: true },
  { id: 'runbooks', label: 'Runbooks', icon: BookOpen, available: true },
  { id: 'equipment', label: 'Equipment', icon: Wrench, available: true },
  { id: 'auth', label: 'Auth', icon: History, available: true },
  { id: 'memory', label: 'Memory', icon: Server, available: true },
  { id: 'chrome', label: 'Chrome', icon: Monitor, available: true },
  { id: 'debug', label: 'Debug', icon: Bug, available: true },
]

type PanelHostProps = {
  initialTab: PanelId
  tasks: unknown[]
  runbooks: unknown[]
  equipment: unknown[]
  authEntries: AuthEntry[]
  compressEntries: CompressEntry[]
  memoryEntries: MemoryEntry[]
  chromeState: ChromeState
  debugEntries: DebugEntry[]
  onClose: () => void
  onToggleEquipment: (id: string) => void
  onCancelTask: (id: string) => void
  onCompleteTask: (id: string) => void
  onToggleMemory?: (id: string, enabled: boolean) => void
  onDeleteMemory?: (id: string) => void
}

export function PanelHost({
  initialTab,
  tasks,
  runbooks,
  equipment,
  authEntries,
  compressEntries,
  memoryEntries,
  chromeState,
  debugEntries,
  onClose,
  onToggleEquipment,
  onCancelTask,
  onCompleteTask,
  onToggleMemory,
  onDeleteMemory,
}: PanelHostProps) {
  const [activeTab, setActiveTab] = useState<PanelId>(initialTab)

  return (
    <div className="modal-backdrop" role="presentation" onMouseDown={(e) => { if (e.target === e.currentTarget) onClose() }}>
      <section className="panel-host" role="dialog" aria-modal="true" aria-label="Panel host">
        <nav className="panel-host-sidebar" aria-label="Panel tabs">
          <div className="panel-host-sidebar-header">
            <Bot size={16} />
            <span>Zhongshu</span>
          </div>
          {tabs.map((tab) => {
            const Icon = tab.icon
            const active = activeTab === tab.id
            return (
              <button
                key={tab.id}
                type="button"
                className={`panel-host-tab${active ? ' active' : ''}${tab.available ? '' : ' coming-soon'}`}
                onClick={() => tab.available && setActiveTab(tab.id)}
                title={tab.available ? tab.label : `${tab.label} — data not yet available`}
              >
                <Icon size={14} />
                <span>{tab.label}</span>
              </button>
            )
          })}
        </nav>

        <div className="panel-host-main">
          <header className="modal-header">
            <h2>{tabs.find((t) => t.id === activeTab)?.label ?? 'Panel'}</h2>
            <button type="button" className="icon-button" aria-label="Close panel" onClick={onClose}>
              <X size={16} />
            </button>
          </header>

          <div className="panel-host-content">
            {activeTab === 'tasks' ? (
              <ItemList items={tasks} kind="tasks" onToggle={onToggleEquipment} onCancel={onCancelTask} onComplete={onCompleteTask} />
            ) : activeTab === 'runbooks' ? (
              <ItemList items={runbooks} kind="runbooks" />
            ) : activeTab === 'equipment' ? (
              <ItemList items={equipment} kind="equipment" onToggle={onToggleEquipment} />
            ) : activeTab === 'auth' ? (
              <AuthHistoryPanel entries={authEntries} />
            ) : activeTab === 'memory' ? (
              <MemoryPanel entries={memoryEntries} onToggle={onToggleMemory} onDelete={onDeleteMemory} />
            ) : activeTab === 'chrome' ? (
              <ChromeStatusCard state={chromeState} />
            ) : activeTab === 'debug' ? (
              <DebugPanel entries={debugEntries} />
            ) : null}
          </div>
        </div>
      </section>
    </div>
  )
}

function ItemList({
  items,
  kind,
  onToggle,
  onCancel,
  onComplete,
}: {
  items: unknown[]
  kind: 'tasks' | 'runbooks' | 'equipment'
  onToggle?: (id: string) => void
  onCancel?: (id: string) => void
  onComplete?: (id: string) => void
}) {
  if (items.length === 0) {
    return (
      <EmptyState
        kind={kind}
        title={kind === 'tasks' ? 'No active tasks' : kind === 'runbooks' ? 'No runbooks yet' : 'No equipment installed'}
        message={
          kind === 'tasks'
            ? 'Background tasks will appear here when the agent starts working.'
            : kind === 'runbooks'
              ? 'Runbooks are generated after tasks complete.'
              : 'Equipment will appear here once installed or discovered.'
        }
      />
    )
  }

  return (
    <div className="resource-list">
      {items.map((item, index) => {
        const id = itemId(item)
        return (
          <article key={id ?? index} className="resource-row">
            <div>
              <strong>{itemTitle(item) ?? `${tabLabel(kind)} ${index + 1}`}</strong>
              <span>{itemSubtitle(item)}</span>
            </div>
            {kind === 'equipment' && id && onToggle ? (
              <button type="button" className="secondary-button" onClick={() => onToggle(id)}>
                Toggle
              </button>
            ) : null}
            {kind === 'tasks' && id ? (
              <div className="resource-actions">
                {onCancel ? <button type="button" className="secondary-button" onClick={() => onCancel(id)}>Cancel</button> : null}
                {onComplete ? <button type="button" className="primary-button" onClick={() => onComplete(id)}>Complete</button> : null}
              </div>
            ) : null}
          </article>
        )
      })}
    </div>
  )
}

function EmptyState({ kind, title, message }: { kind: string; title: string; message: string }) {
  return (
    <div className="panel-empty">
      <div className="panel-empty-icon">
        {kind === 'tasks' ? <Activity size={24} /> : kind === 'runbooks' ? <BookOpen size={24} /> : <Wrench size={24} />}
      </div>
      <strong>{title}</strong>
      <p>{message}</p>
    </div>
  )
}

function ComingSoonPanel({ tabLabel }: { tabLabel: string }) {
  return (
    <div className="panel-empty">
      <div className="panel-empty-icon">
        <Clock size={24} />
      </div>
      <strong>{tabLabel}</strong>
      <p>This panel is not yet available. It will be included in a future update.</p>
    </div>
  )
}

function tabLabel(kind: string): string {
  if (kind === 'tasks') return 'Task'
  if (kind === 'runbooks') return 'Runbook'
  return 'Item'
}

function itemObject(item: unknown): Record<string, unknown> | null {
  return item && typeof item === 'object' ? item as Record<string, unknown> : null
}

function itemId(item: unknown): string | null {
  const object = itemObject(item)
  if (!object) return null
  const value = object.id ?? object.task_id ?? object.name
  return typeof value === 'string' && value ? value : null
}

function itemTitle(item: unknown): string | null {
  const object = itemObject(item)
  if (!object) return typeof item === 'string' ? item : null
  const value = object.title ?? object.name ?? object.id ?? object.task_id
  return typeof value === 'string' && value ? value : null
}

function itemSubtitle(item: unknown): string {
  const object = itemObject(item)
  if (!object) return ''
  const value = object.status ?? object.description ?? object.enabled ?? object.kind
  if (typeof value === 'boolean') return value ? 'enabled' : 'disabled'
  if (typeof value === 'string') return value
  if (typeof value === 'number') return String(value)
  return ''
}
