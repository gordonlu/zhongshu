type ResourceKind = 'tasks' | 'runbooks' | 'equipment'

type ResourceDialogProps = {
  kind: ResourceKind
  items: unknown[]
  onClose: () => void
  onToggleEquipment: (id: string) => void
  onCancelTask: (id: string) => void
  onCompleteTask: (id: string) => void
}

export function ResourceDialog({
  kind,
  items,
  onClose,
  onToggleEquipment,
  onCancelTask,
  onCompleteTask,
}: ResourceDialogProps) {
  return (
    <div className="modal-backdrop" role="presentation">
      <section className="modal-panel resource-panel" role="dialog" aria-modal="true" aria-label={kind}>
        <header className="modal-header">
          <h2>{titleForKind(kind)}</h2>
          <button type="button" className="icon-button" aria-label="Close panel" onClick={onClose}>
            x
          </button>
        </header>

        <div className="resource-list">
          {items.length === 0 ? <p className="muted">No items.</p> : null}
          {items.map((item, index) => {
            const id = itemId(item)
            return (
              <article key={id ?? index} className="resource-row">
                <div>
                  <strong>{itemTitle(item) ?? `${titleForKind(kind)} ${index + 1}`}</strong>
                  <span>{itemSubtitle(item)}</span>
                </div>
                {kind === 'equipment' && id ? (
                  <button type="button" className="secondary-button" onClick={() => onToggleEquipment(id)}>
                    Toggle
                  </button>
                ) : null}
                {kind === 'tasks' && id ? (
                  <div className="resource-actions">
                    <button type="button" className="secondary-button" onClick={() => onCancelTask(id)}>
                      Cancel
                    </button>
                    <button type="button" className="primary-button" onClick={() => onCompleteTask(id)}>
                      Complete
                    </button>
                  </div>
                ) : null}
              </article>
            )
          })}
        </div>
      </section>
    </div>
  )
}

function titleForKind(kind: ResourceKind): string {
  if (kind === 'tasks') return 'Tasks'
  if (kind === 'runbooks') return 'Runbooks'
  return 'Equipment'
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
