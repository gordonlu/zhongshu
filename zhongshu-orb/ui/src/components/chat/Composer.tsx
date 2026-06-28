export function Composer({
  value,
  placeholder,
  onChange,
  onSubmit,
}: {
  value: string
  placeholder: string
  onChange: (value: string) => void
  onSubmit: () => void
}) {
  return (
    <textarea
      data-composer-input
      className="composer-input"
      value={value}
      placeholder={placeholder}
      rows={1}
      onChange={(event) => onChange(event.target.value)}
      onKeyDown={(event) => {
        if (event.key === 'Enter' && !event.shiftKey) {
          event.preventDefault()
          onSubmit()
        }
      }}
    />
  )
}
