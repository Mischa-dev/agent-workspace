import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { AppError } from "../domain/store.js";
const exec = promisify(execFile);
export interface GitHubAdapter {
  capabilities(): Promise<Record<string, unknown>>;
  readPull(url: string): Promise<unknown>;
  publishDraft(input: unknown): Promise<unknown>;
}
export class LocalGitHub implements GitHubAdapter {
  async capabilities() {
    return {
      read: true,
      publish: false,
      reason: "No remote has been authorized for publishing.",
    };
  }
  async readPull(url: string) {
    const m = /^https:\/\/github\.com\/([\w.-]+)\/([\w.-]+)\/pull\/(\d+)$/.exec(
      url,
    );
    if (!m) throw new AppError(400, "Use a GitHub pull request URL.");
    const r = await exec("gh", ["api", `repos/${m[1]}/${m[2]}/pulls/${m[3]}`], {
      timeout: 15000,
      maxBuffer: 1024 * 1024,
    });
    const p = JSON.parse(r.stdout);
    return {
      url: p.html_url,
      title: p.title,
      state: p.state,
      draft: p.draft,
      merged: p.merged,
      headSha: p.head.sha,
      baseSha: p.base.sha,
      author: p.user.login,
      source: "live GitHub",
    };
  }
  async publishDraft() {
    throw new AppError(
      403,
      "Publishing requires explicit authorization for this remote. No branch or PR was published.",
    );
  }
}
export const github = new LocalGitHub();
