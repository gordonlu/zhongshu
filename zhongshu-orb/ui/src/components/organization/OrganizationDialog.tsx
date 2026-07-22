import { useState } from 'react'
import { X } from 'lucide-react'
import type { OrganizationEmployeeInfo } from '../../ipc/events'

type OrganizationDialogProps = {
  objective: string
  employees: OrganizationEmployeeInfo[]
  maxWorkers: number
  onClose: () => void
  onSubmit: (
    employees: OrganizationEmployeeInfo[],
    sequentialHandoff: boolean,
    mutation: boolean,
    fileScopes: { employee: string; owned_files: string[] }[],
  ) => void
}

function parseOwnedFiles(value: string): string[] {
  return [...new Set(value.split(/[\n,]+/).map((file) => file.trim()).filter(Boolean))]
}

function isRelativeOwnedFile(file: string): boolean {
  if (/^(?:[a-zA-Z]:[\\/]|[\\/])/.test(file)) return false
  return !file.split(/[\\/]+/).includes('..')
}

export function OrganizationDialog({
  objective,
  employees,
  maxWorkers,
  onClose,
  onSubmit,
}: OrganizationDialogProps) {
  const [selectedNames, setSelectedNames] = useState<string[]>([])
  const [sequentialHandoff, setSequentialHandoff] = useState(false)
  const [mutation, setMutation] = useState(false)
  const [fileScopeText, setFileScopeText] = useState<Record<string, string>>({})
  const selectedEmployees = employees.filter((employee) => selectedNames.includes(employee.name))
  const fileScopes = selectedEmployees.map((employee) => ({
    employee: employee.name,
    owned_files: parseOwnedFiles(fileScopeText[employee.name] ?? ''),
  }))
  const mutationScopeComplete = !mutation || fileScopes.every((scope) => (
    scope.owned_files.length > 0 && scope.owned_files.every(isRelativeOwnedFile)
  ))

  const isEligible = (employee: OrganizationEmployeeInfo, mutationMode = mutation) => (
    mutationMode
      ? (employee.sandbox_eligible ?? employee.read_only_eligible)
      : employee.read_only_eligible
  )

  const toggleEmployee = (employee: OrganizationEmployeeInfo) => {
    if (!isEligible(employee)) return
    setSelectedNames((current) => {
      if (current.includes(employee.name)) {
        return current.filter((name) => name !== employee.name)
      }
      if (current.length >= maxWorkers) return current
      return [...current, employee.name]
    })
  }

  return (
    <div className="modal-backdrop" role="presentation">
      <section className="modal-panel organization-dialog" role="dialog" aria-modal="true" aria-label="Build organization team">
        <header className="modal-header">
          <div>
            <h2>Build a team</h2>
            <p>{objective}</p>
          </div>
          <button type="button" className="icon-button" aria-label="Close organization" onClick={onClose}>
            <X size={16} />
          </button>
        </header>

        <div className="organization-employee-list">
          {employees.map((employee, index) => {
            const selected = selectedNames.includes(employee.name)
            const eligible = isEligible(employee)
            const blockedBy = mutation
              ? employee.sandbox_blocked_by
              : employee.blocked_by
            return (
              <button
                key={`${employee.name}-${index}`}
                type="button"
                className={selected ? 'organization-employee selected' : 'organization-employee'}
                disabled={!eligible}
                aria-pressed={selected}
                onClick={() => toggleEmployee(employee)}
              >
                <span>
                  <strong>{employee.name}</strong>
                  <small>{employee.role}{employee.focus ? ` · ${employee.focus}` : ''}</small>
                </span>
                <span className="employee-eligibility">
                  {eligible ? (selected ? 'selected' : mutation ? 'sandbox available' : 'available') : `blocked: ${blockedBy ?? 'unsafe tools'}`}
                </span>
              </button>
            )
          })}
          {employees.length === 0 ? <p className="muted">No configured employees.</p> : null}
        </div>

        {mutation && selectedEmployees.length > 0 ? (
          <div className="organization-file-scopes">
            <p>Assign workspace-relative files or directories without “..”. Separate entries with commas.</p>
            {selectedEmployees.map((employee) => (
              <label key={employee.name}>
                <span>{employee.name}</span>
                <input
                  type="text"
                  value={fileScopeText[employee.name] ?? ''}
                  aria-label={`File scope for ${employee.name}`}
                  placeholder="src/module.rs, tests/module.rs"
                  onChange={(event) => {
                    const value = event.target.value
                    setFileScopeText((current) => ({ ...current, [employee.name]: value }))
                  }}
                />
              </label>
            ))}
          </div>
        ) : null}

        <label className="organization-flow-option">
          <input
            type="checkbox"
            checked={sequentialHandoff}
            disabled={selectedEmployees.length < 2}
            onChange={(event) => setSequentialHandoff(event.target.checked)}
          />
          Sequential handoff between selected roles
        </label>
        <label className="organization-flow-option" style={{ opacity: mutation ? 1 : 0.6 }}>
          <input
            type="checkbox"
            checked={mutation}
            onChange={(event) => {
              const nextMutation = event.target.checked
              setMutation(nextMutation)
              setSelectedNames((current) => current.filter((name) => {
                const employee = employees.find((candidate) => candidate.name === name)
                return employee ? isEligible(employee, nextMutation) : false
              }))
            }}
          />
          Mutation mode — employees edit isolated sandboxes; parent Review/Apply controls the workspace
        </label>

        <footer className="modal-actions">
          <span className="muted">{selectedEmployees.length}/{maxWorkers} employees</span>
          <button
            type="button"
            className="primary-button"
            disabled={selectedEmployees.length === 0 || !mutationScopeComplete}
            onClick={() => onSubmit(selectedEmployees, sequentialHandoff, mutation, fileScopes)}
          >
            Assign task
          </button>
        </footer>
      </section>
    </div>
  )
}
