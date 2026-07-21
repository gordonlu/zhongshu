import { useState } from 'react'
import { X } from 'lucide-react'
import type { OrganizationEmployeeInfo } from '../../ipc/events'

type OrganizationDialogProps = {
  objective: string
  employees: OrganizationEmployeeInfo[]
  maxWorkers: number
  onClose: () => void
  onSubmit: (employees: OrganizationEmployeeInfo[], sequentialHandoff: boolean, mutation: boolean) => void
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
  const selectedEmployees = employees.filter((employee) => selectedNames.includes(employee.name))

  const toggleEmployee = (employee: OrganizationEmployeeInfo) => {
    if (!employee.read_only_eligible) return
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
            return (
              <button
                key={`${employee.name}-${index}`}
                type="button"
                className={selected ? 'organization-employee selected' : 'organization-employee'}
                disabled={!employee.read_only_eligible}
                aria-pressed={selected}
                onClick={() => toggleEmployee(employee)}
              >
                <span>
                  <strong>{employee.name}</strong>
                  <small>{employee.role}{employee.focus ? ` · ${employee.focus}` : ''}</small>
                </span>
                <span className="employee-eligibility">
                  {employee.read_only_eligible ? (selected ? 'selected' : 'available') : `blocked: ${employee.blocked_by ?? 'unsafe tools'}`}
                </span>
              </button>
            )
          })}
          {employees.length === 0 ? <p className="muted">No configured employees.</p> : null}
        </div>

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
            onChange={(event) => setMutation(event.target.checked)}
          />
          Mutation mode — employees can modify files
        </label>

        <footer className="modal-actions">
          <span className="muted">{selectedEmployees.length}/{maxWorkers} employees</span>
          <button
            type="button"
            className="primary-button"
            disabled={selectedEmployees.length === 0}
            onClick={() => onSubmit(selectedEmployees, sequentialHandoff, mutation)}
          >
            Assign task
          </button>
        </footer>
      </section>
    </div>
  )
}
