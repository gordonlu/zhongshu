import { forwardRef, useImperativeHandle, useLayoutEffect, useRef } from 'react'

type ComposerProps = {
  value: string
  placeholder: string
  onChange: (value: string) => void
  onSubmit: () => void
}

export const Composer = forwardRef<HTMLTextAreaElement, ComposerProps>(function Composer({
  value,
  placeholder,
  onChange,
  onSubmit,
}, ref) {
  const composingRef = useRef(false)
  const textareaRef = useRef<HTMLTextAreaElement>(null)

  useImperativeHandle(ref, () => textareaRef.current as HTMLTextAreaElement)

  useLayoutEffect(() => {
    const textarea = textareaRef.current
    if (!textarea) return
    textarea.style.height = '0px'
    const height = Math.min(Math.max(textarea.scrollHeight, 38), 132)
    textarea.style.height = `${height}px`
    textarea.style.overflowY = textarea.scrollHeight > 132 ? 'auto' : 'hidden'
  }, [value])

  return (
    <textarea
      ref={textareaRef}
      data-composer-input
      className="composer-input"
      value={value}
      placeholder={placeholder}
      rows={1}
      onChange={(event) => onChange(event.target.value)}
      onCompositionStart={() => {
        composingRef.current = true
      }}
      onCompositionEnd={() => {
        composingRef.current = false
      }}
      onKeyDown={(event) => {
        const nativeEvent = event.nativeEvent as KeyboardEvent
        if (
          event.key === 'Enter'
          && !event.shiftKey
          && !event.ctrlKey
          && !event.altKey
          && !event.metaKey
          && !nativeEvent.isComposing
          && !composingRef.current
        ) {
          event.preventDefault()
          onSubmit()
        }
      }}
    />
  )
})
