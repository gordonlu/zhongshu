import { describe, expect, it } from 'vitest'
import { validateCommand } from './commands'

describe('validateCommand', () => {
  it('rejects empty submit commands', () => {
    expect(validateCommand({ type: 'submit', text: '   ' })).toBe(false)
  })

  it('accepts required command payloads', () => {
    expect(validateCommand({ type: 'approve', request_id: 'req-1' })).toBe(true)
    expect(validateCommand({ type: 'toggle_zoom' })).toBe(true)
    expect(validateCommand({ type: 'start_drag' })).toBe(true)
  })
})
