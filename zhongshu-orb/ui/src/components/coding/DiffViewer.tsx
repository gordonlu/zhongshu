export type DiffLineKind = 'add' | 'remove' | 'hunk' | 'meta' | 'context'

type DiffLine = {
  kind: DiffLineKind
  text: string
  oldLine?: number
  newLine?: number
}

export function DiffViewer({ path, summary }: { path: string; summary: string }) {
  const lines = parseDiffLines(summary)
  const hasDiff = lines.some((line) => line.kind === 'add' || line.kind === 'remove' || line.kind === 'hunk')

  return (
    <section className="diff-viewer" aria-label={`Preview ${path}`}>
      <div className="diff-viewer-header">
        <span>{path}</span>
        <strong>{hasDiff ? `${lines.length} lines` : 'summary'}</strong>
      </div>
      {hasDiff ? (
        <pre className="diff-lines">
          {lines.map((line, index) => (
            <span key={`${index}-${line.kind}`} className={`diff-line ${line.kind}`}>
              <span className="diff-line-number">{line.oldLine ?? ''}</span>
              <span className="diff-line-number">{line.newLine ?? ''}</span>
              <span className="diff-line-code">{line.text || ' '}</span>
            </span>
          ))}
        </pre>
      ) : (
        <p className="diff-summary">{summary || 'No diff preview available yet.'}</p>
      )}
    </section>
  )
}

export function parseDiffLines(summary: string): DiffLine[] {
  let oldLine = 0
  let newLine = 0
  return summary.split(/\r?\n/).map((text) => {
    if (text.startsWith('@@')) {
      const hunk = text.match(/@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/)
      oldLine = hunk ? Number(hunk[1]) : oldLine
      newLine = hunk ? Number(hunk[2]) : newLine
      return { kind: 'hunk', text }
    }

    if (text.startsWith('diff --git') || text.startsWith('index ') || text.startsWith('---') || text.startsWith('+++')) {
      return { kind: 'meta', text }
    }

    if (text.startsWith('+')) {
      return { kind: 'add', text, newLine: newLine++ }
    }

    if (text.startsWith('-')) {
      return { kind: 'remove', text, oldLine: oldLine++ }
    }

    const line = { kind: 'context' as const, text, oldLine, newLine }
    oldLine += 1
    newLine += 1
    return line
  })
}
