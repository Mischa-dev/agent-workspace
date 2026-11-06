// What the tour's busy workspace is made of: a team building a robot fleet's
// telemetry platform. Long names on purpose, so truncation and wrapping show.

export const tourProject = {
  key: 'FLT',
  name: 'Fleet Telemetry & Release Engineering (Atlas platform)',
}

/** Other projects, so the rail and Home look like a workspace that's been used. */
export const otherProjects = [
  { key: 'WEB', name: 'Customer portal' },
  { key: 'OPS', name: 'Field operations and on-call rotation' },
  { key: 'HR', name: 'Hiring 2027' },
]

export type TourPerson = { loginName: string; displayName: string }

export const people: TourPerson[] = [
  { loginName: 'amara', displayName: 'Amara Okonkwo-Hughes' },
  { loginName: 'jonas', displayName: 'Jonas Lindqvist' },
  { loginName: 'priya', displayName: 'Priya Raghunathan' },
  { loginName: 'tomas', displayName: 'Tomás Echeverría Salinas' },
  { loginName: 'hannah', displayName: 'Hannah Whitfield-Moreau' },
  { loginName: 'kenji', displayName: 'Kenji Watanabe' },
  { loginName: 'lea', displayName: 'Léa Fontaine' },
  { loginName: 'dmitri', displayName: 'Dmitri Volkov-Sørensen' },
]

export const agents = [
  { name: 'Muse', description: 'Writes and reviews docs and release notes' },
  { name: 'Scout', description: 'Triages new bugs and finds duplicates' },
  { name: 'Release Steward', description: 'Cuts releases, watches CI and drives PRs to green' },
  { name: 'Telemetry Analyst', description: 'Answers questions about the fleet dashboards' },
]

export const channels = [
  'releases',
  'design-crit',
  'incidents',
  'agents-at-work',
  'firmware-and-hardware-integration-for-the-2027-fleet',
]

export const labels = [
  { name: 'bug', color: 'rose' },
  { name: 'firmware', color: 'teal' },
  { name: 'dashboard', color: 'blue' },
  { name: 'ingest', color: 'indigo' },
  { name: 'release', color: 'green' },
  { name: 'customer-reported', color: 'rust' },
  { name: 'tech-debt', color: 'slate' },
  { name: 'design', color: 'orchid' },
]

export const milestones = ['2026.11 release', '2026.12 release', 'Fleet v3 hardware launch']

// About 200 tasks: every area crossed with a few kinds of work, plus some long
// titles. Deterministic, so screenshots of two runs line up.
const areas = [
  'telemetry ingest',
  'battery health dashboard',
  'OTA firmware updates',
  'fleet map',
  'alert routing',
  'device provisioning',
  'release pipeline',
  'customer exports',
  'on-call runbook',
  'sensor calibration',
  'staging environment',
  'billing usage reports',
  'mobile companion app',
  'audit log',
  'data retention',
  'access roles',
  'charging-dock scheduler',
  'map tile cache',
  'incident timeline',
  'docs site',
]
const kinds = [
  (area: string) => `Fix flaky ${area} test on CI`,
  (area: string) => `Write the ${area} design doc`,
  (area: string) => `Cut p95 latency of ${area} below 200 ms`,
  (area: string) => `Add metrics and a dashboard for ${area}`,
  (area: string) => `${area[0].toUpperCase()}${area.slice(1)}: handle devices that drop offline mid-sync`,
  (area: string) => `Migrate ${area} off the legacy queue`,
  (area: string) => `Customer asks for CSV export from ${area}`,
  (area: string) => `Review permissions on ${area}`,
  (area: string) => `Document how ${area} fails over between regions`,
  (area: string) => `Remove the feature flag around ${area}`,
]
const longTitles = [
  'Investigate why roughly 3% of robots in the Rotterdam warehouse report negative battery temperatures after a firmware rollback, and whether it correlates with the dock firmware version',
  'Decide on retention for raw telemetry (30, 90 or 365 days) and write down what each option costs in storage, query speed and what support can still answer for customers',
  'Release 2026.11: cut the branch, freeze strings, run the soak test on the staging fleet for 48 hours, then promote',
]

export const statuses = ['Backlog', 'Ready', 'In Progress', 'Review', 'Done'] as const
const priorities = ['no_priority', 'low', 'medium', 'high', 'urgent'] as const

export type TourTask = {
  title: string
  status: (typeof statuses)[number]
  priority: (typeof priorities)[number]
  /** An index into people, or null for nobody. */
  assignee: number | null
  labels: string[]
  milestone: string | null
}

export function tourTasks(): TourTask[] {
  const titles = [...longTitles]
  // Each area once per kind, interleaved so neighbours in the list differ.
  for (let i = 0; i < areas.length * kinds.length; i += 1) {
    const area = i % areas.length
    titles.push(kinds[(area + Math.floor(i / areas.length)) % kinds.length](areas[area]))
  }
  return titles.slice(0, 203).map((title, index) => {
    // Most work waits in the backlog; a busy middle; a long tail of done.
    const roll = (index * 37) % 100
    const status = roll < 38 ? 'Backlog' : roll < 52 ? 'Ready' : roll < 66 ? 'In Progress' : roll < 74 ? 'Review' : 'Done'
    return {
      title,
      status,
      priority: priorities[(index * 7) % priorities.length],
      assignee: index % 6 === 5 ? null : (index * 3) % people.length,
      labels: [labels[index % labels.length].name, ...(index % 4 === 0 ? [labels[(index + 3) % labels.length].name] : [])],
      milestone: index % 3 === 0 ? milestones[index % milestones.length] : null,
    }
  })
}

/** The task the tour opens: a long description, criteria and comments. */
export const featuredTask = {
  title: 'Battery health dashboard: show cell imbalance per robot, with a 30-day trend and an alert threshold',
  description: `Support keeps getting asked "is this robot's battery about to fail?" and today they answer it by hand from raw telemetry.

## What we want
- A per-robot panel on the battery health dashboard with **cell imbalance** (max minus min cell voltage), a 30-day sparkline and the current value.
- An alert threshold per fleet, defaulting to 45 mV, that raises an incident in #incidents when three readings in a row cross it.
- A link from the alert straight to the robot's panel.

## Out of scope
- Predicting failure dates. That's the ML team's quarter.
- Docks. They report a different schema (see the ingest design doc).

## Notes
The ingest side already stores \`cell_voltages[]\` on every \`battery.sample\` event since firmware 4.12. Older robots send only the pack voltage, so the panel has to say "Not reported by this firmware" rather than draw a flat line.`,
  criteria: [
    { text: 'Panel shows imbalance, a 30-day trend and the current value', done: true },
    { text: 'Fleet-level threshold, defaulting to 45 mV, editable by admins', done: true },
    { text: 'Three readings over the threshold open an incident in #incidents', done: false },
    { text: 'Robots on firmware before 4.12 say "Not reported by this firmware"', done: false },
  ],
  comments: [
    'Ingest has `cell_voltages[]` back to 2026-08-01 for about 80% of the fleet. The rest are on 4.9 and will need the firmware rollout first.',
    "I'd keep the threshold per fleet rather than per robot for now. Per-robot overrides can come later if support asks.",
    'Draft panel is up behind `battery_imbalance_panel`. Screenshots in #design-crit.',
  ],
}

// #releases, the busy channel. Each entry is [author, text]. Authors are an
// index into people, 'me' (the signed-in person) or an agent's name.
export type Line = [author: number | 'me' | string, text: string]

export const releasesChannel: Line[] = [
  [0, 'Morning! Branch cut for **2026.11** is today at 14:00 UTC. Anything that needs to land, land it before then or tag it for 2026.12.'],
  [1, 'OTA rollback fix is in review, should be in by noon.'],
  [2, 'Telemetry ingest is green on staging after the queue migration. p95 is down from 410 ms to 160 ms 🎉'],
  ['Release Steward', 'Checked the 2026.11 milestone: 41 tasks done, 6 in review, 3 still in progress. The three in progress are all on the battery dashboard.'],
  [3, 'Those three are mine. Two are just waiting on copy review, the third is the alert threshold which I can push to 12 if needed.'],
  [0, "Let's push the threshold to 12 then. Better to ship the panel without alerts than rush the alert logic."],
  [4, "Agreed. I'll update the release notes draft."],
  ['Muse', "Release notes draft for 2026.11 is on the page **Release notes: 2026.11**. I left the battery alert out and added a line under \"Coming next\"."],
  [5, 'Small thing: the staging fleet in Rotterdam is still on dock firmware 2.3. Soak test results from there will look worse than they are.'],
  [2, 'Good catch. Can we exclude Rotterdam docks from the soak dashboard, or at least split them out?'],
  [5, 'I can split them out, 20 minutes.'],
  [6, 'Heads up, the customer exports job failed twice overnight with a timeout. Not release-blocking but someone should look before Friday.'],
  ['Scout', "Found two existing tasks that look like the same timeout. Linked them on the newest one and marked the others as duplicates."],
  [7, 'Looking at the exports timeout now. It is the 90-day window query again, it scans the whole partition.'],
  [7, "```sql\nexplain analyze select * from telemetry_daily\nwhere fleet_id = $1 and day >= now() - interval '90 days';\n-- Seq Scan on telemetry_daily_2026_10 (cost=0.00..412883.11)\n```\nMissing the (fleet_id, day) index on the October partition. The partition was created by hand during the incident."],
  [0, 'Ugh, that incident keeps giving. Can you add the index and a check that new partitions get it?'],
  [7, 'On it. Adding a test that fails if any partition is missing an index the parent has.'],
  [1, 'OTA rollback fix merged ✅'],
  ['Release Steward', 'CI for main is green after the OTA merge. Starting the 48-hour soak on the staging fleet now.'],
  ['me', 'Thanks all. Remember the freeze starts at 14:00, strings included.'],
  [3, 'Panel copy reviewed by Léa, merged.'],
  [4, 'Release notes updated with the panel screenshots.'],
  [2, 'Soak hour 2: no new errors. Memory on the ingest workers is flat.'],
  [6, 'Exports job reran fine after the index. 4 minutes instead of 41.'],
  [0, 'Great work everyone. Next check-in at 16:00.'],
]

export const releasesThread = {
  /** Index into releasesChannel of the message the thread hangs off. */
  root: 14,
  replies: [
    [0, 'Was this partition the one from the October 9 incident?'],
    [7, 'Yes. We created it by hand at 3am and copied the columns but not the indexes.'],
    [2, 'Can we make partitions only through the migration tool from now on? Then this class of thing just goes away.'],
    [7, "That's the plan. The new check fails CI if a partition's indexes differ from the parent's."],
    ['Release Steward', 'Opened a pull request with the check and the missing index. CI is running.'],
    [0, 'Approved. Thanks for jumping on it so fast.'],
  ] as Line[],
}

export const designCritChannel: Line[] = [
  [4, 'New battery panel mockups. I tried two layouts: sparkline left of the value, or under it.'],
  [3, 'Under reads better at the narrow width. Left is nicer on desktop.'],
  [6, 'Under, please. Support uses this on tablets in the warehouse.'],
  ['Muse', 'Summary so far: two votes for under (tablets in the warehouse), one for left. No one has objected to the 45 mV default.'],
  [4, 'Going with under. Thanks!'],
]

export const incidentsChannel: Line[] = [
  ['Telemetry Analyst', '🔴 Ingest lag over 5 minutes in eu-west for 12 minutes. 214 robots affected. Auto-opened from the alert rule.'],
  [5, 'Looking. Looks like one Kafka partition is stuck behind a poison message.'],
  [5, 'Skipped the message and filed it. Lag is back under 10 seconds.'],
  ['Telemetry Analyst', '🟢 Recovered. Total impact 19 minutes, no data lost, 214 robots caught up.'],
]

export const dmLines: Line[] = [
  [0, 'Hey, do you have ten minutes before the branch cut? I want to sanity check the release plan with you.'],
  ['me', 'Sure, after standup?'],
  [0, "Perfect. Main question: do we hold 2026.11 for the battery alert or ship without it? I'm leaning ship."],
  ['me', 'Ship. The panel alone answers what support keeps asking. Alerts can follow in 12.'],
  [0, "Great, that's what I told the channel. I'll put it in the decision log too."],
  [0, 'Also, Hannah is out Thursday and Friday, so release notes review needs someone else. Could Muse take a first pass?'],
  ['me', 'Yes, ask Muse in #releases and I will review after.'],
]

export const plan = {
  title: 'Plan: 2026.11 release and the battery health dashboard',
  body: `# Plan: 2026.11 release and the battery health dashboard

The 2026.11 release ships the battery health panel, the OTA rollback fix and the new ingest queue. The battery alert moves to 2026.12 (decided in #releases on Nov 4).

## Goals
1. Support can answer "is this robot's battery about to fail?" without asking engineering.
2. Firmware rollbacks no longer leave robots stuck on the recovery image.
3. Telemetry ingest p95 stays under 200 ms with the fleet at 1,400 robots.

## Timeline

| Date | What | Who |
| --- | --- | --- |
| Nov 4 | Branch cut, string freeze | Amara |
| Nov 4–6 | 48-hour soak on the staging fleet | Release Steward |
| Nov 6 | Release notes final | Muse, then Hannah |
| Nov 7 | Promote to production, 10% → 50% → 100% | Jonas |

## Risks
- **Rotterdam docks on firmware 2.3.** Soak numbers from there look worse than they are. Kenji split them out of the soak dashboard.
- **Hand-made partitions.** The October partition was missing an index; a new CI check catches this from now on.
- **Hannah is out Thursday and Friday.** Muse takes the first pass on release notes.

## Open questions
- Do we turn the battery panel on for every fleet at once, or start with the three pilot customers?
- Who owns the alert threshold default once alerts ship: support or the fleet admins?

> Keep this page current. When something changes, change it here and say so in #releases.`,
}

export const otherPages = [
  { title: 'Release notes: 2026.11', body: '# Release notes: 2026.11\n\n## New\n- **Battery health panel.** Cell imbalance per robot with a 30-day trend.\n\n## Fixed\n- Robots no longer stay on the recovery image after an OTA rollback.\n- Customer exports for 90-day windows are about ten times faster.\n\n## Coming next\n- Battery imbalance alerts (2026.12).' },
  { title: 'On-call runbook: telemetry ingest', body: '# On-call runbook: telemetry ingest\n\n1. Check lag on the ingest dashboard.\n2. If one partition is stuck, look for a poison message and skip it.\n3. File what you skipped as a task with the `ingest` label.' },
  { title: 'Design doc: ingest queue migration', body: '# Design doc: ingest queue migration\n\nWe move telemetry ingest off the legacy queue onto partitioned streams, one partition per fleet region.' },
  { title: 'Meeting notes: weekly fleet review (Nov 3)', body: '# Weekly fleet review (Nov 3)\n\n- 1,388 robots online, 12 in repair.\n- Battery complaints are down 30% since the firmware 4.12 rollout.' },
]
