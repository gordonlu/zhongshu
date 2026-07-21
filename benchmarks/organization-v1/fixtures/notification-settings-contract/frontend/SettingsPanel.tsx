type NotificationSettings = {
  emailNotifications: boolean
}

export function saveNotificationSettings(settings: NotificationSettings) {
  return fetch('/api/settings/notifications', {
    method: 'PUT',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ emailNotifications: settings.emailNotifications }),
  })
}

export function SettingsPanel({ settings }: { settings: NotificationSettings }) {
  return <label>Email alerts <input type="checkbox" defaultChecked={settings.emailNotifications} /></label>
}
