import type { OverlayToUiEvent } from '../ipc/events'

export const demoCodingEvents: OverlayToUiEvent[] = [
  { type: 'mode_change', mode: 'coding' },
  { type: 'state_change', state: 'coding task running' },
  {
    type: 'history',
    has_more: false,
    entries: [
      {
        role: 'User',
        content: 'Polish the desktop assistant UI and keep runtime logic out of React.',
        tool_calls: [],
      },
      {
        role: 'Assistant',
        content: 'I will tighten the shell, chat surface, review workbench, and verification states while preserving the IPC boundary.',
        tool_calls: [
          { name: 'read_ui_components', status: { Done: { success: true } } },
          { name: 'apply_visual_system', status: 'Running' },
        ],
      },
    ],
  },
  {
    type: 'coding',
    event: { kind: 'plan_created', session_id: 'demo-ui', step_count: 4, risk: 'medium' },
  },
  {
    type: 'coding',
    event: { kind: 'plan_step_started', session_id: 'demo-ui', step_id: '1', title: 'Define the desktop assistant visual system' },
  },
  {
    type: 'coding',
    event: { kind: 'plan_step_completed', session_id: 'demo-ui', step_id: '1', status: 'done' },
  },
  {
    type: 'coding',
    event: { kind: 'plan_step_started', session_id: 'demo-ui', step_id: '2', title: 'Refactor chat and workbench surfaces' },
  },
  {
    type: 'coding',
    event: {
      kind: 'worker_started',
      session_id: 'demo-ui',
      worker: 'deepseek-worker',
      task_id: 'ui-css-pass',
      owned_files: ['zhongshu-orb/ui/src/styles.css'],
    },
  },
  {
    type: 'coding',
    event: { kind: 'context_pressure', pressure_percent: 42, dropped_evidence: 0, dropped_recent: 0 },
  },
  {
    type: 'coding',
    event: { kind: 'context_included', description: 'React overlay shell and IPC bridge', estimated_tokens: 1820 },
  },
  {
    type: 'coding',
    event: {
      kind: 'patch_preview',
      session_id: 'demo-ui',
      path: 'zhongshu-orb/ui/src/styles.css',
      operation: 'modify',
      diff_summary: 'Refine desktop shell, workbench panels, focus states, and composer treatment.',
      diff: {
        summary: 'Refine desktop shell, workbench panels, focus states, and composer treatment.',
        unified_diff: [
          'diff --git a/zhongshu-orb/ui/src/styles.css b/zhongshu-orb/ui/src/styles.css',
          '@@ -1,4 +1,8 @@',
          '-  --bg: #11151d;',
          '+  --bg: #0c1018;',
          '+  --accent: rgb(57, 100, 254);',
          '+  --green: #5bd190;',
        ].join('\n'),
        changed: true,
        replace_all: false,
        removed_lines: 1,
        added_lines: 3,
        before_hash: 'demo-before',
        after_hash: 'demo-after',
      },
    },
  },
  {
    type: 'coding',
    event: { kind: 'verification', command: 'pnpm --dir zhongshu-orb/ui test', success: true, exit_code: 0 },
  },
]
