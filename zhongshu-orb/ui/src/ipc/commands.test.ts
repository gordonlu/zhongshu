import { describe, expect, it } from 'vitest'
import { validateCommand } from './commands'

describe('validateCommand', () => {
  it('rejects empty submit commands', () => {
    expect(validateCommand({ type: 'submit', text: '   ' })).toBe(false)
    expect(validateCommand({ type: 'delegate_review', text: '   ' })).toBe(false)
  })

  it('accepts required command payloads', () => {
    expect(validateCommand({ type: 'approve', request_id: 'req-1' })).toBe(true)
    expect(validateCommand({ type: 'toggle_zoom' })).toBe(true)
    expect(validateCommand({ type: 'start_drag' })).toBe(true)
    expect(validateCommand({ type: 'delegate_review', text: 'review this' })).toBe(true)
    expect(validateCommand({
      type: 'delegate_organization',
      task: {
        objective: 'review cash flow',
        requirements: [{
          role: 'management_accountant',
          capabilities: ['cash_flow_forecasting'],
          responsibility: 'prepare forecast',
          required: true,
        }],
        sequential_handoff: false,
      },
    })).toBe(true)
  })
})
