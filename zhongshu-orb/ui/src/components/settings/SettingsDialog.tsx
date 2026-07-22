import { useEffect, useState } from 'react'
import { X } from 'lucide-react'
import type { SettingsConfig } from '../../ipc/events'

const PERSONALITY_OPTIONS = ['古典', '极客', '温度']

type SettingsDialogProps = {
  config: SettingsConfig
  onClose: () => void
  onSave: (config: SettingsConfig) => void
  onDeleteHistory: () => void
  onClearCache?: () => void
}

function fmtTokens(n: number): string {
  if (n >= 1_000_000) return `${n / 1_000_000}M`
  return `${n / 1_000}K`
}

export function SettingsDialog({ config, onClose, onSave, onDeleteHistory, onClearCache }: SettingsDialogProps) {
  const [draft, setDraft] = useState<SettingsConfig>(config)
  const [confirmDelete, setConfirmDelete] = useState(false)
  const [search, setSearch] = useState('')

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

        <div className="settings-search">
          <input
            type="search"
            placeholder="Search settings..."
            value={search}
            onChange={(event) => setSearch(event.target.value)}
          />
        </div>
        <div className="settings-grid">
          <label data-search={search ? (search && !'API base'.toLowerCase().includes(search.toLowerCase()) ? 'hide' : '') : ''} className={search && !'API base'.toLowerCase().includes(search.toLowerCase()) ? 'setting-hidden' : ''}>
            API base
            <input
              value={draft.api_base}
              onChange={(event) => setDraft({ ...draft, api_base: event.target.value })}
            />
          </label>
          <label className={search && !'Model'.toLowerCase().includes(search.toLowerCase()) ? 'setting-hidden' : ''}>
            Model
            <input
              value={draft.model}
              onChange={(event) => setDraft({ ...draft, model: event.target.value })}
            />
          </label>
          <label className={search && !'Personality'.toLowerCase().includes(search.toLowerCase()) ? 'setting-hidden' : ''}>
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
          <label className={search && !'API key'.toLowerCase().includes(search.toLowerCase()) ? 'setting-hidden' : ''}>
            API key
            <input
              type="password"
              value={draft.api_key}
              placeholder={draft.api_key_saved ? 'Saved in native key store' : ''}
              onChange={(event) => setDraft({ ...draft, api_key: event.target.value })}
            />
            <span className="settings-key-source">
              {draft.api_key
                ? draft.api_key_saved
                  ? 'From system credential store'
                  : 'From DEEPSEEK_API_KEY env var'
                : 'Not configured'}
            </span>
          </label>
          <label className={search && !'Mode'.toLowerCase().includes(search.toLowerCase()) ? 'setting-hidden' : ''}>
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
          <label className={`settings-slider-label${search && !'Max context tokens'.toLowerCase().includes(search.toLowerCase()) ? ' setting-hidden' : ''}`}>
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
          <label className={search && !'Proxy port'.toLowerCase().includes(search.toLowerCase()) ? 'setting-hidden' : ''}>
            Proxy port
            <input
              value={draft.proxy_port ?? ''}
              onChange={(event) => setDraft({ ...draft, proxy_port: event.target.value || undefined })}
            />
          </label>
          <label className={search && !'Background interval'.toLowerCase().includes(search.toLowerCase()) ? 'setting-hidden' : ''}>
            Background interval
            <input
              value={draft.bg_interval ?? ''}
              onChange={(event) => setDraft({ ...draft, bg_interval: event.target.value || undefined })}
            />
          </label>
          <label className={`settings-checkbox${search && !'Background work'.toLowerCase().includes(search.toLowerCase()) ? ' setting-hidden' : ''}`}>
            <input
              type="checkbox"
              checked={draft.bg_enabled ?? false}
              onChange={(event) => setDraft({ ...draft, bg_enabled: event.target.checked })}
            />
            Background work
          </label>
          <label className={`settings-checkbox${search && !'Self-evolving equipment'.toLowerCase().includes(search.toLowerCase()) ? ' setting-hidden' : ''}`}>
            <input
              type="checkbox"
              checked={draft.auto_evolve ?? false}
              onChange={(event) => setDraft({ ...draft, auto_evolve: event.target.checked })}
            />
            Self-evolving equipment
          </label>
          <label className={`settings-checkbox settings-wide${search && !'Intelligent multi-agent'.toLowerCase().includes(search.toLowerCase()) ? ' setting-hidden' : ''}`}>
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
          <label className={`settings-wide${search && !'Background prompt'.toLowerCase().includes(search.toLowerCase()) ? ' setting-hidden' : ''}`}>
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
              <span className="confirm-delete-note">This removes conversation history. Associated memory entries are not deleted.</span>
              <button type="button" className="danger-button" onClick={() => { onDeleteHistory(); setConfirmDelete(false) }}>
                Yes, delete
              </button>
              <button type="button" className="secondary-button" onClick={() => setConfirmDelete(false)}>
                Cancel
              </button>
            </div>
          ) : (
            <>
              <button type="button" className="danger-button" onClick={() => setConfirmDelete(true)}>
                Delete history
              </button>
              {onClearCache ? (
                <button type="button" className="secondary-button" onClick={onClearCache}>
                  Clear cached data
                </button>
              ) : null}
            </>
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
