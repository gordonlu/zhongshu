import { useEffect, useState } from 'react'
import { X } from 'lucide-react'
import type { SettingsConfig } from '../../ipc/events'

type SettingsDialogProps = {
  config: SettingsConfig
  onClose: () => void
  onSave: (config: SettingsConfig) => void
  onDeleteHistory: () => void
}

export function SettingsDialog({ config, onClose, onSave, onDeleteHistory }: SettingsDialogProps) {
  const [draft, setDraft] = useState<SettingsConfig>(config)

  useEffect(() => {
    setDraft(config)
  }, [config])

  return (
    <div className="modal-backdrop" role="presentation">
      <section className="modal-panel settings-panel" role="dialog" aria-modal="true" aria-label="Settings">
        <header className="modal-header">
          <h2>Settings</h2>
          <button type="button" className="icon-button" aria-label="Close settings" onClick={onClose}>
            <X size={16} />
          </button>
        </header>

        <div className="settings-grid">
          <label>
            API base
            <input
              value={draft.api_base}
              onChange={(event) => setDraft({ ...draft, api_base: event.target.value })}
            />
          </label>
          <label>
            Model
            <input
              value={draft.model}
              onChange={(event) => setDraft({ ...draft, model: event.target.value })}
            />
          </label>
          <label>
            Personality
            <input
              value={draft.personality}
              onChange={(event) => setDraft({ ...draft, personality: event.target.value })}
            />
          </label>
          <label>
            API key
            <input
              type="password"
              value={draft.api_key}
              placeholder={draft.api_key_saved ? 'Saved in native key store' : ''}
              onChange={(event) => setDraft({ ...draft, api_key: event.target.value })}
            />
          </label>
          <label>
            Mode
            <select
              value={draft.mode ?? 'assistant'}
              onChange={(event) => setDraft({ ...draft, mode: event.target.value })}
            >
              <option value="assistant">Assistant</option>
              <option value="coding">Coding</option>
            </select>
          </label>
          <label>
            Max context tokens
            <input
              type="number"
              min={0}
              value={draft.max_context_tokens ?? ''}
              onChange={(event) => {
                const value = event.target.value.trim()
                setDraft({
                  ...draft,
                  max_context_tokens: value ? Number(value) : undefined,
                })
              }}
            />
          </label>
          <label>
            Proxy port
            <input
              value={draft.proxy_port ?? ''}
              onChange={(event) => setDraft({ ...draft, proxy_port: event.target.value || undefined })}
            />
          </label>
          <label>
            Background interval
            <input
              value={draft.bg_interval ?? ''}
              onChange={(event) => setDraft({ ...draft, bg_interval: event.target.value || undefined })}
            />
          </label>
          <label className="settings-checkbox">
            <input
              type="checkbox"
              checked={draft.bg_enabled ?? false}
              onChange={(event) => setDraft({ ...draft, bg_enabled: event.target.checked })}
            />
            Background work
          </label>
          <label className="settings-checkbox">
            <input
              type="checkbox"
              checked={draft.auto_evolve ?? false}
              onChange={(event) => setDraft({ ...draft, auto_evolve: event.target.checked })}
            />
            Self-evolving equipment
          </label>
          <label className="settings-wide">
            Background prompt
            <textarea
              value={draft.bg_prompt ?? ''}
              rows={3}
              onChange={(event) => setDraft({ ...draft, bg_prompt: event.target.value || undefined })}
            />
          </label>
        </div>

        <footer className="modal-actions">
          <button type="button" className="danger-button" onClick={onDeleteHistory}>
            Delete history
          </button>
          <button type="button" className="secondary-button" onClick={onClose}>
            Cancel
          </button>
          <button type="button" className="primary-button" onClick={() => onSave(draft)}>
            Save
          </button>
        </footer>
      </section>
    </div>
  )
}
