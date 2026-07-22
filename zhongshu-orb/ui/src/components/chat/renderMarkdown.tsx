import { useState } from 'react'
import type { ReactNode } from 'react'

type MarkdownToken =
  | { type: 'heading'; level: number; text: string }
  | { type: 'paragraph'; text: string }
  | { type: 'code'; lang: string; code: string }
  | { type: 'hr' }
  | { type: 'blockquote'; text: string }
  | { type: 'list'; ordered: boolean; items: string[] }
  | { type: 'table'; headers: string[]; rows: string[][] }

function tokenizeLine(line: string): MarkdownToken | null {
  const heading = line.match(/^(#{1,6})\s+(.+)$/)
  if (heading) return { type: 'heading', level: heading[1].length, text: heading[2] }

  if (/^---+\s*$/.test(line) || /^\*\*\*+\s*$/.test(line) || /^___+\s*$/.test(line))
    return { type: 'hr' }

  const blockquote = line.match(/^>\s?(.*)$/)
  if (blockquote) return { type: 'blockquote', text: blockquote[1] }

  return null
}

function renderInline(text: string, codeKey: string): ReactNode[] {
  const nodes: ReactNode[] = []
  let remaining = text
  let keyCounter = 0

  while (remaining) {
    const codeMatch = remaining.match(/^`([^`]+)`/)
    if (codeMatch) {
      nodes.push(<code key={`${codeKey}-code-${keyCounter++}`}>{codeMatch[1]}</code>)
      remaining = remaining.slice(codeMatch[0].length)
      continue
    }

    const boldMatch = remaining.match(/^\*\*(.+?)\*\*/)
    if (boldMatch) {
      nodes.push(<strong key={`${codeKey}-bold-${keyCounter++}`}>{boldMatch[1]}</strong>)
      remaining = remaining.slice(boldMatch[0].length)
      continue
    }

    const linkMatch = remaining.match(/^\[([^\]]+)\]\(([^)]+)\)/)
    if (linkMatch) {
      nodes.push(<a key={`${codeKey}-link-${keyCounter++}`} href={linkMatch[2]}>{linkMatch[1]}</a>)
      remaining = remaining.slice(linkMatch[0].length)
      continue
    }

    const newlineMatch = remaining.match(/^\n/)
    if (newlineMatch) {
      nodes.push(<br key={`${codeKey}-br-${keyCounter++}`} />)
      remaining = remaining.slice(1)
      continue
    }

    nodes.push(remaining[0])
    remaining = remaining.slice(1)
  }

  return nodes
}

function CodeBlock({ lang, code, codeKey }: { lang: string; code: string; codeKey: string }) {
  const [copied, setCopied] = useState(false)
  return (
    <div className="md-code-block-wrap">
      <div className="md-code-block-head">
        {lang ? <span className="md-code-lang">{lang}</span> : <span />}
        <button
          type="button"
          className="md-code-copy"
          onClick={async () => {
            try {
              await navigator.clipboard.writeText(code)
              setCopied(true)
              setTimeout(() => setCopied(false), 2000)
            } catch { /* silent */ }
          }}
        >
          {copied ? 'copied' : 'copy'}
        </button>
      </div>
      <pre className="md-code-block">
        <code>{code}</code>
      </pre>
    </div>
  )
}

function tokenizeTable(lines: string[], startIndex: number): { token: MarkdownToken; consumed: number } | null {
  const first = lines[startIndex]
  const headerMatch = first.match(/^\|(.+)\|$/)
  if (!headerMatch) return null

  const headers = headerMatch[1].split('|').map((cell) => cell.trim()).filter(Boolean)
  if (headers.length === 0) return null

  const second = lines[startIndex + 1]
  if (!second || !/^\|[-| :]+\|$/.test(second.trim())) return null

  const rows: string[][] = []
  let index = startIndex + 2

  while (index < lines.length) {
    const rowMatch = lines[index].match(/^\|(.+)\|$/)
    if (!rowMatch) break
    const cells = rowMatch[1].split('|').map((cell) => cell.trim()).filter(Boolean)
    if (cells.length === 0) break
    rows.push(cells)
    index++
  }

  return {
    token: { type: 'table', headers, rows },
    consumed: index - startIndex,
  }
}

function tokenizeCodeBlock(lines: string[], startIndex: number): { token: MarkdownToken; consumed: number } {
  const first = lines[startIndex]
  const lang = first.startsWith('```') ? first.slice(3).trim() : ''
  const codeLines: string[] = []
  let index = startIndex + 1

  while (index < lines.length && !lines[index].startsWith('```')) {
    codeLines.push(lines[index])
    index++
  }

  return {
    token: { type: 'code', lang, code: codeLines.join('\n') },
    consumed: index - startIndex + 1,
  }
}

function tokenizeList(lines: string[], startIndex: number, ordered: boolean): { token: MarkdownToken; consumed: number } {
  const items: string[] = []
  let index = startIndex

  while (index < lines.length) {
    const line = lines[index]
    if (ordered) {
      const match = line.match(/^\s*\d+\.\s+(.+)$/)
      if (!match) break
      items.push(match[1])
    } else {
      const match = line.match(/^\s*[-*+]\s+(.+)$/)
      if (!match) break
      items.push(match[1])
    }
    index++
  }

  return {
    token: { type: 'list', ordered, items },
    consumed: index - startIndex,
  }
}

export function renderMarkdown(text: string, streaming?: boolean): ReactNode[] {
  const rawLines = text.split('\n')
  const lines = rawLines.map((line) => line.replace(/\r$/, ''))
  const blocks: ReactNode[] = []
  let index = 0
  let keyCounter = 0

  while (index < lines.length) {
    const line = lines[index]
    const trimmed = line.trim()

    if (trimmed === '') {
      index++
      continue
    }

    if (trimmed.startsWith('```')) {
      const { token, consumed } = tokenizeCodeBlock(lines, index)
      const codeKey = `code-${keyCounter++}`
      const codeToken = token as MarkdownToken & { lang: string; code: string }
      blocks.push(
        <CodeBlock key={codeKey} lang={codeToken.lang} code={codeToken.code} codeKey={codeKey} />,
      )
      index += consumed
      continue
    }

    if (trimmed.startsWith('|')) {
      const tableResult = tokenizeTable(lines, index)
      if (tableResult) {
        const { token, consumed } = tableResult
        const tableKey = `table-${keyCounter++}`
        const tableToken = token as MarkdownToken & { headers: string[]; rows: string[][] }
        blocks.push(
          <div key={tableKey} className="md-table-wrap">
            <table className="md-table">
              <thead>
                <tr>{tableToken.headers.map((h, hi) => <th key={`${tableKey}-h-${hi}`}>{renderInline(h, `${tableKey}-h-${hi}`)}</th>)}</tr>
              </thead>
              <tbody>
                {tableToken.rows.map((row, ri) => (
                  <tr key={`${tableKey}-r-${ri}`}>{row.map((cell, ci) => <td key={`${tableKey}-r-${ri}-c-${ci}`}>{renderInline(cell, `${tableKey}-r-${ri}-c-${ci}`)}</td>)}</tr>
                ))}
              </tbody>
            </table>
          </div>,
        )
        index += consumed
        continue
      }
      if (streaming) {
        blocks.push(<div key={`table-ph-${keyCounter++}`} className="md-table-placeholder" />)
        index++
        continue
      }
    }

    if (/^\s*[-*+]\s+/.test(trimmed)) {
      const { token, consumed } = tokenizeList(lines, index, false)
      const listKey = `list-${keyCounter++}`
      const listToken = token as MarkdownToken & { items: string[] }
      blocks.push(
        <ul key={listKey} className="md-list">
          {listToken.items.map((item: string, itemIndex: number) => (
            <li key={`${listKey}-${itemIndex}`}>{renderInline(item, `${listKey}-${itemIndex}`)}</li>
          ))}
        </ul>,
      )
      index += consumed
      continue
    }

    if (/^\s*\d+\.\s+/.test(trimmed)) {
      const { token, consumed } = tokenizeList(lines, index, true)
      const listKey = `ol-${keyCounter++}`
      const listToken = token as MarkdownToken & { items: string[] }
      blocks.push(
        <ol key={listKey} className="md-list">
          {listToken.items.map((item: string, itemIndex: number) => (
            <li key={`${listKey}-${itemIndex}`}>{renderInline(item, `${listKey}-${itemIndex}`)}</li>
          ))}
        </ol>,
      )
      index += consumed
      continue
    }

    const token = tokenizeLine(trimmed)
    if (token) {
      const tokKey = `${token.type}-${keyCounter++}`
      if (token.type === 'hr') {
        blocks.push(<hr key={tokKey} className="md-hr" />)
      } else if (token.type === 'heading') {
        const Tag = `h${Math.min(token.level, 6)}` as keyof JSX.IntrinsicElements
        blocks.push(<Tag key={tokKey} className={`md-heading md-h${token.level}`}>{renderInline(token.text, tokKey)}</Tag>)
      } else if (token.type === 'blockquote') {
        blocks.push(<blockquote key={tokKey} className="md-blockquote">{renderInline(token.text, tokKey)}</blockquote>)
      }
      index++
      continue
    }

    const paraKey = `p-${keyCounter++}`
    blocks.push(<p key={paraKey} className="md-paragraph">{renderInline(trimmed, paraKey)}</p>)
    index++
  }

  return blocks
}
