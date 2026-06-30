import { forwardRef, useRef } from 'react'

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

  return (
    <textarea
      ref={ref}
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
