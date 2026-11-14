import type { Reporter, TestCase } from '@playwright/test/reporter'

/** After a run, lists the test-for-done items still owed (fixme) and who owns each (W1.35). */
export default class OwnersReporter implements Reporter {
  private owed = new Map<string, string>()

  onTestBegin(test: TestCase) {
    this.note(test)
  }

  onTestEnd(test: TestCase) {
    this.note(test)
  }

  private note(test: TestCase) {
    const owner = test.annotations.find((annotation) => annotation.type === 'owner')?.description
    if (owner && test.expectedStatus === 'skipped') this.owed.set(test.title, owner)
  }

  onEnd() {
    if (!this.owed.size) return
    const lines = [...this.owed].sort().map(([item, owner]) => `  ${item} (owner: ${owner})`)
    console.log(`\nTest for "done": items still owed\n${lines.join('\n')}\n`)
  }
}
