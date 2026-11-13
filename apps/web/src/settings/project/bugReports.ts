import { requestJson } from '../../api/client'

/**
 * Bug report links: a secret URL per project that other apps POST bug
 * reports to, each becoming a task. The copy-paste snippets for those apps
 * are built here too, so they're tested apart from the page.
 */
export const bugReportKeys = {
  links: (projectId: string) => ['project', projectId, 'bug-report-links'] as const,
}

export type BugReportLink = {
  id: string
  projectId: string
  name: string
  ownerActorId: string
  ownerName: string | null
  tokenHint: string
  state: 'active' | 'paused' | 'disabled'
  disabledReason: string | null
  reportCount: number
  lastReportAt: string | null
  canManage: boolean
  version: number
  createdAt: string
  /** A collection's form (AWD-110): each answer there becomes a record. */
  collectionId?: string
  collectionName?: string
  form?: FormSettings
}

/** What a form asks: its fields by ID, in order, after the title. */
export type FormSettings = { intro?: string; fields: { field: string; required?: boolean }[]; thanks?: string }

export type BugReportLinkList = { links: BugReportLink[]; canCreate: boolean }
export type BugReportLinkWithUrl = { link: BugReportLink; url: string; formUrl?: string }

const json = <T>(path: string, method: string, body: unknown = {}) =>
  requestJson<T>(path, { method, json: body, idempotencyKey: crypto.randomUUID() })

const linkPath = (linkId: string) => `/api/v1/bug-report-links/${encodeURIComponent(linkId)}`

export const listBugReportLinks = (projectId: string) =>
  requestJson<BugReportLinkList>(`/api/v1/projects/${encodeURIComponent(projectId)}/bug-report-links`)

export const createBugReportLink = (projectId: string, name: string) =>
  json<BugReportLinkWithUrl>(`/api/v1/projects/${encodeURIComponent(projectId)}/bug-report-links`, 'POST', { name })

export const createCollectionForm = (projectId: string, collectionId: string, name: string, form: FormSettings) =>
  json<BugReportLinkWithUrl>(`/api/v1/projects/${encodeURIComponent(projectId)}/bug-report-links`, 'POST', { name, collectionId, form })

export const updateBugReportLink = (linkId: string, change: { name?: string; state?: 'active' | 'paused'; form?: FormSettings }) =>
  json<BugReportLink>(linkPath(linkId), 'PATCH', change)

export const replaceBugReportLink = (linkId: string) => json<BugReportLinkWithUrl>(`${linkPath(linkId)}/replace`, 'POST')

export const deleteBugReportLink = (linkId: string) => requestJson<void>(linkPath(linkId), { method: 'DELETE' })

export type SnippetKind = 'agent' | 'curl' | 'javascript' | 'python'

export const SNIPPET_KINDS: { value: SnippetKind; label: string }[] = [
  { value: 'agent', label: 'For an agent' },
  { value: 'curl', label: 'curl' },
  { value: 'javascript', label: 'JavaScript' },
  { value: 'python', label: 'Python' },
]

/** What to paste into the other app: instructions for an agent working on it, or code. */
export function snippet(kind: SnippetKind, url: string, appName: string, projectName: string): string {
  switch (kind) {
    case 'agent':
      return [
        `Add a "report a bug" path to ${appName} that sends bug reports to my task board (the ${projectName} project).`,
        '',
        `Send each report as an HTTP POST to this URL. Treat it like a password: keep it in config or an environment variable (BUG_REPORT_URL), not in a public repository.`,
        url,
        '',
        'The body is JSON. Only "title" or "description" is needed:',
        '{"title": "Short summary", "description": "What happened and how to make it happen again",',
        ' "priority": "low | medium | high | urgent", "id": "optional id; a resend with the same id makes no second task"}',
        'Any other fields (app version, platform, user, page or level, stack trace, logs) are welcome: they are listed under "Details" on the task. Keep a report under 64 KB.',
        '',
        'It answers 201 with {"ok": true, "task": "KEY-12"}, or 200 with "duplicate": true when the same report (or id) came in the last day. Errors come back as {"ok": false, "error": "..."}; 429 means slow down. CORS is open, so a web page can call it directly; navigator.sendBeacon(url, JSON.stringify(report)) works too.',
        '',
        'Please: send reports for crashes and unhandled errors automatically (one per distinct error, not one per frame), add a simple way for a player or user to describe a problem and send it, include the app version and platform, and never let a failed report break the app.',
      ].join('\n')
    case 'curl':
      return [
        `curl -X POST '${url}' \\`,
        "  -H 'Content-Type: application/json' \\",
        `  -d '{"title": "The save button does nothing", "description": "Clicked save on the settings page, nothing happened.", "version": "1.4.2"}'`,
      ].join('\n')
    case 'javascript':
      return [
        `const BUG_REPORT_URL = '${url}'`,
        '',
        '// Sends a bug report; never throws.',
        'export function reportBug(report) {',
        '  const body = JSON.stringify(report)',
        "  if (typeof navigator !== 'undefined' && navigator.sendBeacon?.(BUG_REPORT_URL, body)) return",
        "  fetch(BUG_REPORT_URL, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body, keepalive: true }).catch(() => {})",
        '}',
        '',
        '// Crashes and unhandled errors, reported on their own.',
        "if (typeof window !== 'undefined') {",
        "  window.addEventListener('error', (event) =>",
        '    reportBug({ title: String(event.message), stack: event.error?.stack, page: location.href, userAgent: navigator.userAgent }))',
        "  window.addEventListener('unhandledrejection', (event) =>",
        "    reportBug({ title: 'Unhandled promise rejection: ' + String(event.reason), stack: event.reason?.stack, page: location.href }))",
        '}',
      ].join('\n')
    case 'python':
      return [
        'import json, urllib.request',
        '',
        `BUG_REPORT_URL = "${url}"`,
        '',
        'def report_bug(title, description="", **details):',
        '    """Sends a bug report; never raises."""',
        '    body = json.dumps({"title": title, "description": description, **details}).encode()',
        '    request = urllib.request.Request(BUG_REPORT_URL, data=body, headers={"Content-Type": "application/json"})',
        '    try:',
        '        urllib.request.urlopen(request, timeout=5)',
        '    except Exception:',
        '        pass',
      ].join('\n')
  }
}
