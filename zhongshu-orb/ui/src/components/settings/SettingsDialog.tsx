import { useEffect, useState } from 'react'
import { X } from 'lucide-react'
import type { SettingsConfig } from '../../ipc/events'

const PERSONALITY_OPTIONS = ['古典', '极客', '温度']

type SettingsDialogProps = {
  config: SettingsConfig
  onClose: () => void
  onSave: (config: SettingsConfig) => void
  onDeleteHistory: () => void
}

function fmtTokens(n: number): string {
  if (n >= 1_000_000) return `${n / 1_000_000}M`
  return `${n / 1_000}K`
}

export function SettingsDialog({ config, onClose, onSave, onDeleteHistory }: SettingsDialogProps) {
  const [draft, setDraft] = useState<SettingsConfig>(config)
  const [confirmDelete, setConfirmDelete] = useState(false)

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
            <select
              value={PERSONALITY_OPTIONS.includes(draft.personality) ? draft.personality : ''}
              onChange={(event) => setDraft({ ...draft, personality: event.target.value })}
              className="settings-select"
            >
              <option value="" disabled>Select personality</option>
              {PERSONALITY_OPTIONS.map((personality) => (
                <option key={personality} value={personality}>{personality}</option>
              ))}
            </select>
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
              className="settings-select"
            >
              <option value="assistant">Assistant</option>
              <option value="coding">Coding</option>
            </select>
          </label>
          <label className="settings-slider-label">
            <span>Max context tokens <strong>{draft.max_context_tokens ? fmtTokens(draft.max_context_tokens) : ''}</strong></span>
            <input
              type="range"
              min={500000}
              max={1000000}
              step={100000}
              value={draft.max_context_tokens ?? 500000}
              onChange={(event) => {
                setDraft({
                  ...draft,
                  max_context_tokens: Number(event.target.value),
                })
              }}
              className="settings-slider"
            />
            <div className="settings-slider-labels">
              <span>500K</span>
              <span>1M</span>
            </div>
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
          <label className="settings-checkbox settings-wide">
            <input
              type="checkbox"
              checked={draft.auto_multi_agent ?? false}
              onChange={(event) => setDraft((current) => ({
                ...current,
                auto_multi_agent: event.target.checked,
              }))}
            />
            <span>
              Intelligent multi-agent orchestration
              <small>Allow Zhongshu to form a bounded team when specialization or parallel work is worthwhile.</small>
            </span>
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
          {confirmDelete ? (
            <div className="confirm-delete">
              <span>Are you sure?</span>
              <button type="button" className="danger-button" onClick={() => { onDeleteHistory(); setConfirmDelete(false) }}>
                Yes, delete
              </button>
              <button type="button" className="secondary-button" onClick={() => setConfirmDelete(false)}>
                Cancel
              </button>
            </div>
          ) : (
            <button type="button" className="danger-button" onClick={() => setConfirmDelete(true)}>
              Delete history
            </button>
          )}
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
