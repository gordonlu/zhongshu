import { useState } from 'react'
import { Bot, CheckCircle2, Key, Link, Loader2 } from 'lucide-react'
import type { SettingsConfig } from '../../ipc/events'

type StartupGateProps = {
  config: SettingsConfig
  onSave: (config: SettingsConfig) => void
}

export function StartupGate({ config, onSave }: StartupGateProps) {
  const [apiKey, setApiKey] = useState(config.api_key)
  const [apiBase, setApiBase] = useState(config.api_base || 'https://api.deepseek.com')
  const [model, setModel] = useState(config.model || 'deepseek-chat')
  const [saving, setSaving] = useState(false)

  const handleSave = () => {
    setSaving(true)
    onSave({
      ...config,
      api_key: apiKey,
      api_base: apiBase,
      model,
    })
  }

  const hasEnvVar = !config.api_key_saved && !!config.api_key

  return (
    <div className="startup-gate">
      <div className="startup-gate-panel">
        <div className="startup-gate-header">
          <Bot size={32} />
          <h1>Welcome to Zhongshu</h1>
          <p>Configure your API access to get started.</p>
        </div>

        {hasEnvVar ? (
          <div className="startup-gate-detected">
            <CheckCircle2 size={16} />
            <span>API key detected from <code>DEEPSEEK_API_KEY</code> environment variable</span>
          </div>
        ) : null}

        <div className="startup-gate-form">
          <label className="startup-field">
            <span><Key size={12} /> API Key</span>
            <input
              type="password"
              value={apiKey}
              placeholder={hasEnvVar ? 'Using env var — override here' : 'sk-...'}
              onChange={(e) => setApiKey(e.target.value)}
              autoFocus
            />
          </label>

          <label className="startup-field">
            <span><Link size={12} /> API Base URL</span>
            <input
              type="text"
              value={apiBase}
              placeholder="https://api.deepseek.com"
              onChange={(e) => setApiBase(e.target.value)}
            />
          </label>

          <label className="startup-field">
            <span><Bot size={12} /> Model</span>
            <input
              type="text"
              value={model}
              placeholder="deepseek-chat"
              onChange={(e) => setModel(e.target.value)}
            />
          </label>
        </div>

        <div className="startup-gate-info">
          <p>Your API key is stored in the system credential store. It is never shared or logged.</p>
        </div>

        <div className="startup-gate-actions">
          <button
            type="button"
            className="primary-button"
            disabled={!apiKey.trim() || saving}
            onClick={handleSave}
          >
            {saving ? <Loader2 size={14} className="spin" /> : null}
            {saving ? 'Saving...' : 'Continue'}
          </button>
        </div>
      </div>
    </div>
  )
}
