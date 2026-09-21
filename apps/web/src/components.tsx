import React, { useEffect, useRef, useState } from "react";
import {
  ArrowLeft,
  ArrowRight,
  Check,
  CheckCircle2,
  ChevronDown,
  ChevronRight,
  Circle,
  Clock,
  Download,
  FileText,
  FolderOpen,
  GitBranch,
  Layers,
  Link2,
  Loader2,
  MessageSquare,
  Pause,
  Play,
  Plus,
  RefreshCw,
  Settings,
  Square,
  Upload,
  Users,
  X,
  AlertCircle,
  ExternalLink,
  Copy,
} from "lucide-react";
import type { RecordRow, Attachment } from "../../../packages/shared/src/index";

import {
  PlanningViews,
  AgentConnection,
  ResourceAttachmentForm,
  ResourceOrganizationForm,
  SpaceExistingForm,
} from "./connected";
type Row = RecordRow<any>;
type Props = Record<string, any>;
const list = (rows: Row[], kind: string) => rows.filter((r) => r.kind === kind);
const find = (rows: Row[], id?: string) => rows.find((r) => r.id === id);
const lines = (value: string) =>
  value
    .split("\n")
    .map((v) => v.trim())
    .filter(Boolean);
const label = (status: string) => status?.replaceAll("_", " ") || "Unknown";
const timestamp = (value?: string) =>
  value
    ? new Date(value).toLocaleString(undefined, {
        month: "short",
        day: "numeric",
        hour: "numeric",
        minute: "2-digit",
      })
    : "Not recorded";
const attempt = (fn: () => Promise<any>) => {
  void fn().catch(() => {});
};

function useOperation() {
  const [pending, setPending] = useState(false),
    [error, setError] = useState("");
  const lock = useRef(false);
  async function run(fn: () => Promise<any>) {
    if (lock.current) return;
    lock.current = true;
    setPending(true);
    setError("");
    try {
      return await fn();
    } catch (e: any) {
      setError(
        e.message || "This could not be saved. Your changes are still here.",
      );
    } finally {
      lock.current = false;
      setPending(false);
    }
  }
  return { pending, error, run };
}

export function RichText({ text }: { text: string }) {
  const chunks = String(text || "").split(/(```[\s\S]*?```)/g);
  const inline = (value: string) =>
    value
      .split(/(\*\*[^*]+\*\*|`[^`]+`|\[[^\]]+\]\(https?:\/\/[^\s)]+\))/g)
      .map((part, i) => {
        if (part.startsWith("**") && part.endsWith("**"))
          return <strong key={i}>{part.slice(2, -2)}</strong>;
        if (part.startsWith("`") && part.endsWith("`"))
          return <code key={i}>{part.slice(1, -1)}</code>;
        const link = part.match(/^\[([^\]]+)\]\((https?:\/\/[^\s)]+)\)$/);
        if (link)
          return (
            <a key={i} href={link[2]} target="_blank" rel="noreferrer">
              {link[1]}
            </a>
          );
        return part;
      });
  return (
    <div className="rich-text">
      {chunks.map((chunk, i) =>
        chunk.startsWith("```") ? (
          <pre key={i}>
            <code>
              {chunk.replace(/^```[^\n]*\n?/, "").replace(/```$/, "")}
            </code>
          </pre>
        ) : (
          chunk.split("\n").map((line, j) =>
            /^#{1,4}\s/.test(line) ? (
              <h4 key={`${i}-${j}`}>{inline(line.replace(/^#+\s/, ""))}</h4>
            ) : /^[-*]\s/.test(line) ? (
              <div className="text-list-item" key={`${i}-${j}`}>
                <span>•</span>
                <span>{inline(line.slice(2))}</span>
              </div>
            ) : (
              <div key={`${i}-${j}`}>{line ? inline(line) : <br />}</div>
            ),
          )
        ),
      )}
    </div>
  );
}

export function Status({ value }: { value: string }) {
  return (
    <span className={`status status-${value}`}>
      {["accepted", "completed", "complete"].includes(value) ? (
        <CheckCircle2 size={12} />
      ) : ["running", "active", "dispatching"].includes(value) ? (
        <Loader2 className="spin" size={12} />
      ) : value === "review" ? (
        <Circle size={12} />
      ) : (
        <span className="status-dot" />
      )}
      {label(value)}
    </span>
  );
}
function ErrorNote({ error }: { error: string }) {
  return error ? (
    <div role="alert" className="form-error">
      <AlertCircle size={16} />
      <span>{error}</span>
    </div>
  ) : null;
}
function Submit({ children, pending, ...props }: Props) {
  return (
    <button className="primary" type="submit" disabled={pending} {...props}>
      {pending && <Loader2 className="spin" size={15} />} {children}
    </button>
  );
}
function Field({ label: text, hint, children }: Props) {
  return (
    <label className="field">
      <span>{text}</span>
      {children}
      {hint && <small>{hint}</small>}
    </label>
  );
}
function Checks({
  items,
  selected,
  setSelected,
  title,
}: {
  items: { id: string; title: string; detail?: string }[];
  selected: string[];
  setSelected: (v: string[]) => void;
  title?: string;
}) {
  return (
    <fieldset className="check-list">
      {title && <legend>{title}</legend>}
      {items.map((item) => (
        <label className="check-option" key={item.id}>
          <input
            type="checkbox"
            checked={selected.includes(item.id)}
            onChange={(e) =>
              setSelected(
                e.target.checked
                  ? [...selected, item.id]
                  : selected.filter((id) => id !== item.id),
              )
            }
          />
          <span>
            {item.title}
            {item.detail && <small>{item.detail}</small>}
          </span>
        </label>
      ))}
      {!items.length && <p className="muted small">Nothing here yet.</p>}
    </fieldset>
  );
}

function taskReasons(task: Row, rows: Row[]) {
  const t = task.data,
    reasons: string[] = [];
  if (!t.selected) reasons.push("Alternative not selected");
  if (
    t.execution === "code" &&
    t.assigneeId &&
    (find(rows, t.assigneeId)?.data.deliveryMode || "managed") !== "managed"
  )
    reasons.push("Choose a local agent for repository execution");
  if (t.blocker) reasons.push(t.blocker);
  if (!t.description?.trim()) reasons.push("Add an outcome");
  if (!t.criteria?.length) reasons.push("Add acceptance criteria");
  for (const id of t.dependencies || []) {
    const d = find(rows, id);
    if (d?.data.status !== "accepted")
      reasons.push(`Waiting for ${d?.data.title || "a prerequisite"}`);
    else if (d.data.execution === "code" && !d.data.integratedCommit)
      reasons.push(`Integration needed: ${d.data.title}`);
  }
  return reasons;
}

export function TaskRow({
  task,
  onClick,
  depth = 0,
  reasons = [],
  children,
}: Props) {
  return (
    <div
      className={`task-row-wrap ${task.data.selected === false ? "unselected" : ""}`}
    >
      <button
        className="task-row"
        style={{ "--task-depth": depth } as React.CSSProperties}
        onClick={onClick}
      >
        <span
          className={`task-check ${task.data.status === "accepted" ? "checked" : ""}`}
        >
          {task.data.status === "accepted" ? (
            <Check size={13} />
          ) : task.data.status === "running" ? (
            <Loader2 size={13} className="spin" />
          ) : null}
        </span>
        <span className="task-title">
          <strong>{task.data.title}</strong>
          {(reasons.length > 0 || task.data.waitingReason) && (
            <small>
              {reasons[0] || task.data.waitingReason}
              {reasons.length > 1 ? ` +${reasons.length - 1}` : ""}
            </small>
          )}
        </span>
        <span className="task-kind">{task.data.kind}</span>
        {task.data.execution === "code" && <GitBranch size={14} />}
        <Status value={task.data.status} />
        <ChevronRight size={14} />
      </button>
      {children}
    </div>
  );
}

export function ProjectView({
  project,
  rows,
  command,
  setPanel,
  openModal,
  back,
  search,
}: Props) {
  const [tab, setTab] = useState("Outline"),
    [filter, setFilter] = useState("all"),
    [collapsed, setCollapsed] = useState<string[]>([]);
  const tasks = list(rows, "task")
      .filter((t) => t.data.projectId === project.id)
      .sort((a, b) => a.data.position - b.data.position),
    done = tasks.filter((t) => t.data.status === "accepted").length;
  const proposals = list(rows, "proposal").filter(
    (p) => p.data.projectId === project.id && p.data.status === "proposed",
  );
  const sessions = list(rows, "workSession").filter((s) =>
    s.data.taskIds.some((id: string) => tasks.some((t) => t.id === id)),
  );
  const renderTasks = (parent?: string, depth = 0): React.ReactNode =>
    tasks
      .filter((t) =>
        parent
          ? t.data.parentId === parent
          : !t.data.parentId || !tasks.some((p) => p.id === t.data.parentId),
      )
      .map((t) => {
        const visible =
          (!search ||
            `${t.data.title} ${t.data.description}`
              .toLowerCase()
              .includes(search.toLowerCase())) &&
          (filter === "all" || t.data.status === filter);
        return (
          <React.Fragment key={t.id}>
            {visible && (
              <div
                className="outline-node"
                style={
                  {
                    "--outline-depth": Math.min(depth, 8),
                  } as React.CSSProperties
                }
              >
                {tasks.some((child) => child.data.parentId === t.id) && (
                  <button
                    className="tree-toggle"
                    aria-label={`${collapsed.includes(t.id) ? "Expand" : "Collapse"} ${t.data.title}`}
                    aria-expanded={!collapsed.includes(t.id)}
                    onClick={() =>
                      setCollapsed(
                        collapsed.includes(t.id)
                          ? collapsed.filter((id) => id !== t.id)
                          : [...collapsed, t.id],
                      )
                    }
                  >
                    {collapsed.includes(t.id) ? (
                      <ChevronRight size={13} />
                    ) : (
                      <ChevronDown size={13} />
                    )}
                  </button>
                )}
                <TaskRow
                  task={t}
                  depth={Math.min(depth, 8)}
                  onClick={() => setPanel(t)}
                  reasons={taskReasons(t, rows)}
                />
              </div>
            )}{" "}
            {depth < 25 &&
              (search || filter !== "all" || !collapsed.includes(t.id)) &&
              renderTasks(t.id, depth + 1)}
          </React.Fragment>
        );
      });
  return (
    <>
      <button className="back-link" onClick={back}>
        <ArrowLeft size={15} /> All projects
      </button>
      <div className="project-heading">
        <div className="project-heading-icon">
          <FolderOpen size={25} />
        </div>
        <div>
          <p className="eyebrow">
            {find(rows, project.data.spaceId)?.data.name || "YOUR PROJECT"}
          </p>
          <h1>{project.data.name}</h1>
        </div>
        <button
          className="quiet"
          onClick={() => openModal({ type: "project", edit: project })}
        >
          <Settings size={16} /> Project settings
        </button>
      </div>
      <p className="project-description">
        {project.data.description ||
          "Add an outcome to give this project a direction."}
      </p>
      <div className="project-summary">
        <div>
          <strong>{done}</strong>
          <span>accepted</span>
        </div>
        <div>
          <strong>
            {tasks.filter((t) => t.data.status === "review").length}
          </strong>
          <span>to review</span>
        </div>
        <div>
          <strong>
            {
              tasks.filter(
                (t) =>
                  taskReasons(t, rows).length &&
                  !["accepted", "cancelled"].includes(t.data.status),
              ).length
            }
          </strong>
          <span>need preparation</span>
        </div>
        <div className="project-progress">
          <span>
            {done} / {tasks.length} tasks
          </span>
          <div className="progress-track">
            <span
              style={{
                width: `${tasks.length ? (done / tasks.length) * 100 : 0}%`,
              }}
            />
          </div>
        </div>
      </div>
      <div className="view-toolbar">
        <div className="tabs">
          {["Outline", "Board", "Dependencies", "Resources", "Activity"].map(
            (t) => (
              <button
                key={t}
                className={tab === t ? "active" : ""}
                onClick={() => setTab(t)}
              >
                {t}
              </button>
            ),
          )}
        </div>
        <div className="toolbar-actions">
          <button
            className="secondary"
            onClick={() => openModal({ type: "plan", project })}
          >
            <MessageSquare size={15} /> Plan with an agent
          </button>
          <button
            className="primary"
            onClick={() => openModal({ type: "task", projectId: project.id })}
          >
            <Plus size={15} /> Add task
          </button>
        </div>
      </div>
      {["Outline", "Board", "Dependencies"].includes(tab) && (
        <>
          <div className="outline-toolbar">
            <select
              aria-label="Filter tasks by status"
              value={filter}
              onChange={(e) => setFilter(e.target.value)}
            >
              <option value="all">All tasks</option>
              {[
                "draft",
                "ready",
                "running",
                "review",
                "accepted",
                "cancelled",
              ].map((s) => (
                <option key={s} value={s}>
                  {label(s)}
                </option>
              ))}
            </select>
            <span className="muted small">
              Grouping keeps the plan readable. Dependencies control what can
              start.
            </span>
            <button
              className="secondary"
              disabled={!tasks.length}
              onClick={() =>
                openModal({ type: "scope", projectId: project.id })
              }
            >
              <Play size={14} /> Start work session
            </button>
          </div>
          {proposals.map((p) => (
            <button
              className="proposal-banner"
              key={p.id}
              onClick={() => setPanel(p)}
            >
              <Layers size={20} />
              <span>
                <strong>{p.data.title}</strong>
                <small>
                  {p.data.tasks.length} suggested tasks · awaiting your review
                </small>
              </span>
              <span>
                Review proposal <ArrowRight size={14} />
              </span>
            </button>
          ))}
          {tab === "Outline" ? (
            <>
              <div className="task-list">
                {renderTasks()}
                {!tasks.length && (
                  <div className="inline-empty">
                    <FolderOpen size={26} />
                    <h3>Every big project starts somewhere.</h3>
                    <p>
                      Capture a first task, or ask an agent to suggest a plan
                      you can review.
                    </p>
                    <button
                      className="secondary"
                      onClick={() =>
                        openModal({ type: "task", projectId: project.id })
                      }
                    >
                      <Plus size={15} /> Add the first task
                    </button>
                  </div>
                )}
              </div>
              <button
                className="add-task-row"
                onClick={() =>
                  openModal({ type: "task", projectId: project.id })
                }
              >
                <Plus size={15} /> Add task
              </button>
            </>
          ) : (
            <PlanningViews
              view={tab}
              tasks={tasks}
              rows={rows}
              search={search}
              filter={filter}
              command={command}
              setPanel={setPanel}
              project={project}
              openModal={openModal}
            />
          )}
        </>
      )}
      {tab === "Resources" && (
        <>
          <div className="outline-toolbar">
            <p className="muted small">
              Linked material keeps its original identity and revision history.
            </p>
            <button
              className="secondary"
              onClick={() =>
                openModal({ type: "resource", projectId: project.id })
              }
            >
              <Plus size={14} /> Add Resource
            </button>
          </div>
          {list(rows, "resource")
            .filter((r) => r.data.projectIds.includes(project.id))
            .map((r) => (
              <button
                key={r.id}
                className="resource-list-row"
                onClick={() => setPanel(r)}
              >
                <FileText size={18} />
                <span>
                  <strong>{r.data.title}</strong>
                  <small>
                    {r.data.type} · revision{" "}
                    {find(rows, r.data.latestRevisionId)?.data.number}
                  </small>
                </span>
                <ChevronRight size={14} />
              </button>
            ))}
        </>
      )}
      {tab === "Activity" && (
        <>
          <div className="outline-toolbar">
            <p className="muted small">
              Bounded sessions, preserved results, and linked pull requests.
            </p>
            <button
              className="secondary"
              onClick={() => openModal({ type: "link", projectId: project.id })}
            >
              <Link2 size={14} /> Link pull request
            </button>
          </div>
          {list(rows, "scope")
            .filter(
              (s) =>
                s.data.taskIds.some((id: string) =>
                  tasks.some((t) => t.id === id),
                ) && !sessions.some((session) => session.data.scopeId === s.id),
            )
            .map((s) => (
              <div className="session-card" key={s.id}>
                <header>
                  <div>
                    <strong>{s.data.title}</strong>
                    <small>Approved {timestamp(s.data.approvedAt)}</small>
                  </div>
                  <Status value="ready" />
                </header>
                <p>
                  {s.data.taskIds.length} tasks · {s.data.minutes} minutes ·{" "}
                  {s.data.maxWorkers} workers
                </p>
                <button
                  className="secondary"
                  onClick={() =>
                    attempt(() => command("session.start", { scopeId: s.id }))
                  }
                >
                  <Play size={13} /> Start approved scope
                </button>
                <details>
                  <summary>Local trigger</summary>
                  <code className="path">pnpm trigger {s.id}</code>
                </details>
              </div>
            ))}
          {sessions.map((s) => (
            <div className="session-card" key={s.id}>
              <header>
                <div>
                  <strong>
                    {find(rows, s.data.scopeId)?.data.title || "Work session"}
                  </strong>
                  <small>{timestamp(s.data.startedAt)}</small>
                </div>
                <Status value={s.data.status} />
              </header>
              <p>
                {s.data.taskIds.length} approved tasks · up to{" "}
                {s.data.maxWorkers} workers · ends {timestamp(s.data.deadline)}
              </p>
              {["active", "stopping"].includes(s.data.status) && (
                <button
                  className="secondary"
                  disabled={s.data.status === "stopping"}
                  onClick={() =>
                    attempt(() => command("session.stop", { id: s.id }))
                  }
                >
                  <Square size={13} />
                  {s.data.status === "stopping" ? "Stopping…" : "Stop session"}
                </button>
              )}
              {list(rows, "run")
                .filter((r) => r.data.sessionId === s.id)
                .map((r) => (
                  <button
                    className="run-row"
                    key={r.id}
                    onClick={() => setPanel(r)}
                  >
                    <span>
                      {find(rows, r.data.taskId)?.data.title || "Task run"}
                    </span>
                    <Status value={r.data.status} />
                    <ChevronRight size={14} />
                  </button>
                ))}
            </div>
          ))}
          {list(rows, "link")
            .filter((l) => l.data.projectId === project.id)
            .map((l) => (
              <PullLink key={l.id} row={l} />
            ))}
          {!sessions.length && (
            <div className="inline-empty">
              <Clock size={25} />
              <h3>No work sessions yet</h3>
              <p>
                Prepare tasks, then review and approve a bounded scope from the
                Outline.
              </p>
            </div>
          )}
        </>
      )}
    </>
  );
}

export function DetailPanel({
  row,
  rows,
  command,
  close,
  open,
  name,
  auth,
  setNotice,
}: Props) {
  const title =
    row.kind === "run"
      ? "Execution record"
      : row.kind === "conversation"
        ? "Conversation context"
        : row.kind === "proposal"
          ? "Review proposal"
          : row.kind === "task"
            ? "Task details"
            : "Resource";
  return (
    <aside className="detail-panel">
      <header className="panel-header">
        <span>{title}</span>
        <button aria-label="Close details" onClick={close}>
          <X size={19} />
        </button>
      </header>
      <div className="panel-content">
        {row.kind === "resource" ? (
          <ResourcePanel
            key={`${row.id}:${row.viewRevisionId || ""}`}
            row={row}
            rows={rows}
            command={command}
            setNotice={setNotice}
          />
        ) : row.kind === "task" ? (
          <TaskPanel
            key={row.id}
            row={row}
            rows={rows}
            command={command}
            open={open}
          />
        ) : row.kind === "proposal" ? (
          <ProposalPanel key={row.id} row={row} rows={rows} command={command} />
        ) : row.kind === "conversation" ? (
          <ConversationPanel
            key={row.id}
            row={row}
            rows={rows}
            command={command}
            open={open}
          />
        ) : row.kind === "run" ? (
          <RunPanel key={row.id} row={row} rows={rows} command={command} />
        ) : (
          <pre>{JSON.stringify(row.data, null, 2)}</pre>
        )}
      </div>
    </aside>
  );
}

function fileSize(bytes: number) {
  if (bytes < 1024) return `${bytes} bytes`;
  return `${Number((bytes / 1024).toFixed(1))} KB`;
}

function ResourcePanel({ row, rows, command, setNotice }: Props) {
  const current = find(rows, row.data.latestRevisionId),
    draftKey = `resource-draft:${row.id}`;
  const savedDraft = () => {
    try {
      return JSON.parse(localStorage.getItem(draftKey) || "null");
    } catch {
      return null;
    }
  };
  const [draft, setDraft] = useState(savedDraft),
    [title, setTitle] = useState(draft?.title ?? row.data.title),
    [text, setText] = useState(draft?.text ?? current?.data.text ?? ""),
    [base, setBase] = useState(draft?.base ?? row.revision),
    [editing, setEditing] = useState(!!draft && !row.viewRevisionId),
    [selectedRevision, setSelectedRevision] = useState(
      row.viewRevisionId || row.data.latestRevisionId,
    ),
    [sharing, setSharing] = useState(false);
  const [conversationIds, setConversations] = useState<string[]>(
      row.data.conversationIds,
    ),
    [projectIds, setProjects] = useState<string[]>(row.data.projectIds),
    [spaceIds, setSpaces] = useState<string[]>(row.data.spaceIds),
    [agentIds, setAgents] = useState<string[]>(row.data.agentIds);
  const revision = find(rows, selectedRevision) || current,
    revisions = list(rows, "revision")
      .filter((r) => r.data.resourceId === row.id)
      .sort((a, b) => b.data.number - a.data.number),
    op = useOperation();
  useEffect(() => {
    if (editing)
      localStorage.setItem(draftKey, JSON.stringify({ title, text, base }));
  }, [editing, title, text, base, draftKey]);
  const beginEdit = () => {
    if (!draft) {
      setText(current?.data.text || "");
      setTitle(row.data.title);
      setBase(row.revision);
    }
    setEditing(true);
  };
  return (
    <>
      <div className="detail-heading">
        <span className="resource-icon">
          <FileText size={23} />
        </span>
        <p className="eyebrow">{row.data.type}</p>
        <h2>{row.data.title}</h2>
      </div>
      {row.data.stagedFor && (
        <p className="notice-box">
          An independent opinion created this private draft. It becomes shared
          when that round finishes.
        </p>
      )}
      {draft && !editing && (
        <div className="pinned-notice">
          <span>A saved draft is available.</span>
          <button onClick={() => setEditing(true)}>Resume draft</button>
        </div>
      )}
      {selectedRevision !== row.data.latestRevisionId && (
        <div className="pinned-notice">
          <span>Viewing a pinned revision.</span>
          <button
            onClick={() => setSelectedRevision(row.data.latestRevisionId)}
          >
            View latest
          </button>
        </div>
      )}
      <div className="resource-meta">
        <select
          aria-label="Resource revision"
          value={selectedRevision}
          onChange={(e) => setSelectedRevision(e.target.value)}
        >
          {revisions.map((r) => (
            <option key={r.id} value={r.id}>
              Revision {r.data.number}
              {r.id === row.data.latestRevisionId ? " · latest" : ""}
            </option>
          ))}
        </select>
        <span>{timestamp(revision?.createdAt)}</span>
      </div>
      {editing ? (
        <form
          onSubmit={(e) => {
            e.preventDefault();
            op.run(async () => {
              const result = await command("resource.revise", {
                id: row.id,
                expectedRevision: base,
                title,
                text,
              });
              setBase(result.resource.revision);
              setSelectedRevision(result.revision.id);
              setEditing(false);
              setDraft(null);
              localStorage.removeItem(draftKey);
              setNotice(
                "New Resource revision saved. Existing attachments keep their version.",
              );
            });
          }}
        >
          <Field label="Title">
            <input
              value={title}
              onChange={(e) => setTitle(e.target.value)}
              required
              maxLength={240}
            />
          </Field>
          <Field label="Content">
            <textarea
              className="document-editor"
              value={text}
              onChange={(e) => setText(e.target.value)}
              maxLength={256000}
            />
          </Field>
          {base !== row.revision && (
            <div className="notice-box">
              This Resource changed while you were editing. Your draft is
              preserved.
              <button
                type="button"
                className="inline-link"
                onClick={() => {
                  setBase(row.revision);
                  setNotice(
                    "Current revision selected as the base. Review both versions before saving.",
                  );
                }}
              >
                Use current revision as the new base
              </button>
              <details>
                <summary>View current saved content</summary>
                <RichText text={current?.data.text || ""} />
              </details>
            </div>
          )}
          <ErrorNote error={op.error} />
          <div className="form-actions">
            <button
              type="button"
              className="secondary"
              onClick={() => {
                setEditing(false);
                setDraft(null);
                localStorage.removeItem(draftKey);
              }}
            >
              Discard draft
            </button>
            <Submit pending={op.pending}>Save new revision</Submit>
          </div>
        </form>
      ) : (
        <>
          {revision?.data.storageKey ? (
            <div className="file-preview">
              <FileText size={35} />
              <strong>{revision.data.filename}</strong>
              <small>
                {fileSize(revision.data.size)} · {revision.data.mimeType}
              </small>
              <a
                className="secondary"
                href={`/api/resources/${row.id}/revisions/${revision.id}/download`}
                download
              >
                <Download size={15} /> Download file
              </a>
            </div>
          ) : (
            <div className="document-body">
              <RichText text={revision?.data.text || "This note is empty."} />
            </div>
          )}
          <div className="form-actions">
            {row.data.type !== "file" && (
              <button className="secondary" onClick={beginEdit}>
                Edit latest revision
              </button>
            )}
            <button
              className="secondary"
              onClick={() => {
                setConversations(row.data.conversationIds);
                setProjects(row.data.projectIds);
                setSpaces(row.data.spaceIds);
                setAgents(row.data.agentIds);
                setSharing(!sharing);
              }}
            >
              <Users size={15} /> Sharing & places
            </button>
          </div>
        </>
      )}
      {sharing && (
        <form
          className="subform"
          onSubmit={(e) => {
            e.preventDefault();
            op.run(async () => {
              await command("resource.grant", {
                id: row.id,
                expectedRevision: row.revision,
                conversationIds,
                projectIds,
                spaceIds,
                agentIds,
              });
              setSharing(false);
              setNotice("Resource access updated.");
            });
          }}
        >
          <h3>Who can use this Resource?</h3>
          <p className="muted small">
            Agents need access both here and in the conversation. Linking a
            project or space only organizes the Resource.
          </p>
          <Checks
            title="Conversations"
            items={list(rows, "conversation").map((r) => ({
              id: r.id,
              title: r.data.title,
            }))}
            selected={conversationIds}
            setSelected={(ids) => {
              const newIds = ids.filter((id) => !conversationIds.includes(id));
              setConversations(ids);
              setAgents([
                ...new Set([
                  ...agentIds,
                  ...newIds.flatMap(
                    (id) => find(rows, id)?.data.agentIds || [],
                  ),
                ]),
              ]);
            }}
          />
          <Checks
            title="Agents"
            items={list(rows, "agent").map((r) => ({
              id: r.id,
              title: r.data.name,
            }))}
            selected={agentIds}
            setSelected={setAgents}
          />
          <Checks
            title="Projects"
            items={list(rows, "project").map((r) => ({
              id: r.id,
              title: r.data.name,
            }))}
            selected={projectIds}
            setSelected={setProjects}
          />
          <Checks
            title="Spaces"
            items={list(rows, "space").map((r) => ({
              id: r.id,
              title: r.data.name,
            }))}
            selected={spaceIds}
            setSelected={setSpaces}
          />
          <ErrorNote error={op.error} />
          <Submit pending={op.pending}>Save access</Submit>
        </form>
      )}
      <details className="technical-details">
        <summary>Revision details</summary>
        <dl>
          <dt>Author</dt>
          <dd>
            {revision?.data.authorId === "owner"
              ? "You"
              : find(rows, revision?.data.authorId)?.data.name || "Agent"}
          </dd>
          <dt>Size</dt>
          <dd>{revision?.data.size} bytes</dd>
          <dt>Checksum</dt>
          <dd>
            <code>{revision?.data.checksum}</code>
          </dd>
          <dt>Resource ID</dt>
          <dd>
            <code>{row.id}</code>
          </dd>
        </dl>
      </details>
    </>
  );
}

function TaskPanel({ row, rows, command, open }: Props) {
  const [editing, setEditing] = useState(false),
    op = useOperation(),
    r = row.data,
    reasons = taskReasons(row, rows),
    run = find(rows, r.runId);
  return (
    <>
      <div className="detail-heading">
        <div className="flex">
          <span className="eyebrow">
            {r.kind} · {r.execution === "code" ? "code task" : "agent task"}
          </span>
          <Status value={r.status} />
        </div>
        <h2>{r.title}</h2>
      </div>
      {editing ? (
        <TaskForm
          key={`edit:${row.id}`}
          rows={rows}
          task={row}
          command={command}
          onDone={() => setEditing(false)}
        />
      ) : (
        <>
          <div className="document-body">
            <RichText text={r.description || "No outcome written yet."} />
          </div>
          <h3>Acceptance criteria</h3>
          {r.criteria?.length ? (
            <ul className="criteria">
              {r.criteria.map((v: string, n: number) => (
                <li key={n}>
                  <Square size={13} />
                  {v}
                </li>
              ))}
            </ul>
          ) : (
            <p className="muted small">
              Add criteria before approving this task for execution.
            </p>
          )}
          {reasons.length > 0 &&
            !["accepted", "cancelled"].includes(r.status) && (
              <div className="notice-box">
                <strong>Before this can start</strong>
                <ul>
                  {reasons.map((reason) => (
                    <li key={reason}>{reason}</li>
                  ))}
                </ul>
              </div>
            )}
          <div className="task-properties">
            <div>
              <span>Assigned agent</span>
              <strong>
                {find(rows, r.assigneeId)?.data.name ||
                  "Choose when work starts"}
              </strong>
            </div>
            <div>
              <span>Project</span>
              <strong>
                {find(rows, r.projectId)?.data.name || "Loose task"}
              </strong>
            </div>
            <div>
              <span>Parent</span>
              <strong>{find(rows, r.parentId)?.data.title || "None"}</strong>
            </div>
            <div>
              <span>Worktree</span>
              <strong>
                {r.execution === "code"
                  ? r.parallel
                    ? "Isolated · parallel allowed"
                    : "Exclusive"
                  : "Not needed"}
              </strong>
            </div>
            <div>
              <span>Alternative</span>
              <strong>{r.selected ? "Selected" : "Unselected"}</strong>
            </div>
          </div>
          {r.dependencies?.length > 0 && (
            <>
              <h3>Depends on</h3>
              {r.dependencies.map((id: string) => {
                const d = find(rows, id);
                return (
                  d && (
                    <button
                      className="related-row"
                      key={id}
                      onClick={() => open(d)}
                    >
                      <Link2 size={14} />
                      <span>{d.data.title}</span>
                      <Status value={d.data.status} />
                    </button>
                  )
                );
              })}
            </>
          )}
          {r.waitingReason && (
            <p className="notice-box">
              <Clock size={14} />
              {r.waitingReason}
            </p>
          )}
          {r.outputResources?.length > 0 && (
            <>
              <h3>Saved output</h3>
              {r.outputResources.map((pin: Attachment) => {
                const resource = find(rows, pin.resourceId);
                return (
                  resource && (
                    <button
                      className="related-row"
                      key={pin.revisionId}
                      onClick={() =>
                        open({ ...resource, viewRevisionId: pin.revisionId })
                      }
                    >
                      <FileText size={14} />
                      <span>{resource.data.title}</span>
                      <small>v{find(rows, pin.revisionId)?.data.number}</small>
                    </button>
                  )
                );
              })}
            </>
          )}
          {r.resources?.length > 0 && (
            <>
              <h3>Pinned Resources</h3>
              {r.resources.map((p: Attachment) => {
                const res = find(rows, p.resourceId);
                return (
                  res && (
                    <button
                      className="related-row"
                      key={p.revisionId}
                      onClick={() =>
                        open({ ...res, viewRevisionId: p.revisionId })
                      }
                    >
                      <FileText size={14} />
                      <span>{res.data.title}</span>
                      <small>v{find(rows, p.revisionId)?.data.number}</small>
                    </button>
                  )
                );
              })}
            </>
          )}
          {r.locks?.length > 0 && (
            <p className="muted small">Shared locks: {r.locks.join(", ")}</p>
          )}
          {run && (
            <>
              <h3>Latest result</h3>
              <button className="related-row" onClick={() => open(run)}>
                <GitBranch size={16} />
                <span>View execution record</span>
                <Status value={run.data.status} />
                <ChevronRight size={14} />
              </button>
              {run.data.output && (
                <details className="result-preview">
                  <summary>Read output</summary>
                  <RichText text={run.data.output} />
                </details>
              )}
            </>
          )}
          {r.priorRunIds?.length > 0 && (
            <details>
              <summary>Previous attempts</summary>
              {r.priorRunIds.map((id: string) => {
                const prior = find(rows, id);
                return (
                  prior && (
                    <button
                      className="related-row"
                      key={id}
                      onClick={() => open(prior)}
                    >
                      <span>{timestamp(prior.createdAt)}</span>
                      <Status value={prior.data.status} />
                    </button>
                  )
                );
              })}
            </details>
          )}
          {r.commit && (
            <p className="muted small">
              Produced commit <code>{r.commit}</code>
            </p>
          )}
          {r.integratedCommit && (
            <p className="notice-box success">
              Verified in the target branch: <code>{r.integratedCommit}</code>
            </p>
          )}
          <ErrorNote error={op.error} />
          <div className="form-actions">
            {r.status !== "running" && (
              <button className="secondary" onClick={() => setEditing(true)}>
                Edit task
              </button>
            )}
            {r.status === "review" && (
              <Submit
                pending={op.pending}
                onClick={() =>
                  op.run(() =>
                    command("task.update", {
                      id: row.id,
                      expectedRevision: row.revision,
                      status: "accepted",
                    }),
                  )
                }
              >
                <Check size={15} /> Accept reviewed work
              </Submit>
            )}
            {run &&
              ["failed", "cancelled", "completed"].includes(run.data.status) &&
              r.status !== "accepted" && (
                <button
                  className="secondary"
                  disabled={op.pending}
                  onClick={() =>
                    op.run(() =>
                      command("task.retry", {
                        id: row.id,
                        expectedRevision: row.revision,
                      }),
                    )
                  }
                >
                  <RefreshCw size={14} /> Prepare another attempt
                </button>
              )}
            {r.status === "draft" && reasons.length === 0 && (
              <button
                className="secondary"
                onClick={() =>
                  op.run(() =>
                    command("task.update", {
                      id: row.id,
                      expectedRevision: row.revision,
                      status: "ready",
                    }),
                  )
                }
              >
                {r.assigneeId &&
                (find(rows, r.assigneeId)?.data.deliveryMode || "managed") !==
                  "managed"
                  ? `Send to ${find(rows, r.assigneeId)?.data.name} inbox`
                  : "Mark ready"}
              </button>
            )}
          </div>
          {r.status === "accepted" &&
            r.execution === "code" &&
            !r.integratedCommit && (
              <div className="notice-box">
                Acceptance is recorded. Code dependents wait until the produced
                commit is verified in the project's target branch.
                <button
                  className="secondary"
                  disabled={op.pending}
                  onClick={() =>
                    op.run(() =>
                      command("task.verifyIntegration", {
                        id: row.id,
                        expectedRevision: row.revision,
                      }),
                    )
                  }
                >
                  <RefreshCw size={14} /> Check integration
                </button>
              </div>
            )}
          {list(rows, "link")
            .filter((l) => l.data.taskId === row.id)
            .map((l) => (
              <PullLink key={l.id} row={l} />
            ))}
        </>
      )}
    </>
  );
}

function ProposalPanel({ row, rows, command }: Props) {
  const [selected, setSelected] = useState<string[]>(
      row.data.tasks.map((_: any, i: number) => String(i)),
    ),
    op = useOperation();
  return (
    <>
      <p className="eyebrow">
        {find(rows, row.data.projectId)?.data.name || "SUGGESTED PLAN"}
      </p>
      <h2>{row.data.title}</h2>
      <p className="muted">
        Select the tasks you want to add. Applying a proposal creates draft
        tasks. Execution needs a separate approval.
      </p>
      <Status value={row.data.status} />
      <div className="proposal-items">
        {row.data.tasks.map((t: any, i: number) => (
          <article key={i}>
            <label className="check-option">
              <input
                type="checkbox"
                disabled={row.data.status !== "proposed"}
                checked={selected.includes(String(i))}
                onChange={(e) =>
                  setSelected(
                    e.target.checked
                      ? [...selected, String(i)]
                      : selected.filter((v) => v !== String(i)),
                  )
                }
              />
              <span>
                <strong>{t.title}</strong>
                <small>
                  {t.kind || "feature"} · {t.execution || "chat"}
                </small>
              </span>
            </label>
            <div className="proposal-task-body">
              <RichText text={t.description || "Outcome needs detail."} />
              {t.criteria?.length > 0 && (
                <ul>
                  {t.criteria.map((c: string, j: number) => (
                    <li key={j}>{c}</li>
                  ))}
                </ul>
              )}
              {t.dependencyIndexes?.length > 0 && (
                <p className="muted small">
                  Depends on{" "}
                  {t.dependencyIndexes
                    .map(
                      (j: number) =>
                        row.data.tasks[j]?.title || `item ${j + 1}`,
                    )
                    .join(", ")}
                </p>
              )}
            </div>
          </article>
        ))}
      </div>
      <ErrorNote error={op.error} />
      {row.data.status === "proposed" && (
        <div className="sticky-actions">
          <button
            className="secondary"
            disabled={op.pending}
            onClick={() =>
              op.run(() =>
                command("proposal.dismiss", {
                  id: row.id,
                  expectedRevision: row.revision,
                }),
              )
            }
          >
            Dismiss
          </button>
          <Submit
            pending={op.pending}
            disabled={!selected.length || op.pending}
            onClick={() =>
              op.run(() =>
                command("proposal.apply", {
                  id: row.id,
                  expectedRevision: row.revision,
                  selectedIndexes: selected.map(Number),
                }),
              )
            }
          >
            Add {selected.length} draft tasks
          </Submit>
        </div>
      )}
    </>
  );
}

function ConversationPanel({ row, rows, command, open }: Props) {
  const [title, setTitle] = useState(row.data.title),
    [agents, setAgents] = useState<string[]>(row.data.agentIds),
    [projectId, setProject] = useState(row.data.projectId || ""),
    [spaceId, setSpace] = useState(row.data.spaceId || ""),
    op = useOperation();
  const burst = find(rows, row.data.burstId),
    reqs = list(rows, "request").filter(
      (r) => r.data.conversationId === row.id,
    );
  return (
    <>
      <h2>{row.data.title}</h2>
      <p className="muted">
        Each conversation has its own history and scoped agent sessions. Other
        conversations are not included.
      </p>
      <form
        onSubmit={(e) => {
          e.preventDefault();
          op.run(() =>
            command("conversation.update", {
              id: row.id,
              expectedRevision: row.revision,
              title,
              agentIds: agents,
              projectId: projectId || null,
              spaceId: spaceId || null,
            }),
          );
        }}
      >
        <Field label="Conversation name">
          <input
            value={title}
            onChange={(e) => setTitle(e.target.value)}
            required
          />
        </Field>
        {row.data.kind === "thread" ? (
          <p className="muted small">
            Participants and project context follow the parent conversation.
          </p>
        ) : (
          <Checks
            title="Participants"
            items={list(rows, "agent").map((a) => ({
              id: a.id,
              title: a.data.name,
              detail: a.data.role,
            }))}
            selected={agents}
            setSelected={setAgents}
          />
        )}
        <Field label="Project context">
          <select
            value={projectId}
            disabled={row.data.kind === "thread"}
            onChange={(e) => setProject(e.target.value)}
          >
            <option value="">No project</option>
            {list(rows, "project").map((p) => (
              <option value={p.id} key={p.id}>
                {p.data.name}
              </option>
            ))}
          </select>
        </Field>
        <Field label="Space">
          <select
            disabled={row.data.kind === "thread"}
            value={spaceId}
            onChange={(e) => setSpace(e.target.value)}
          >
            <option value="">No space</option>
            {list(rows, "space").map((s) => (
              <option value={s.id} key={s.id}>
                {s.data.name}
              </option>
            ))}
          </select>
        </Field>
        <ErrorNote error={op.error} />
        <Submit pending={op.pending} disabled={!agents.length || op.pending}>
          Save conversation
        </Submit>
      </form>
      <h3>Current exchange</h3>
      <dl className="simple-details">
        <dt>Focus</dt>
        <dd>
          {find(rows, row.data.focusAgentId)?.data.name ||
            "Choose with Auto or an @recipient"}
        </dd>
        <dt>Autonomous turns left</dt>
        <dd>{burst?.data.remaining ?? "A new request allows up to six"}</dd>
        <dt>State</dt>
        <dd>
          {row.data.stopped ? "Stopped" : label(burst?.data.mode || "Ready")}
        </dd>
      </dl>
      {burst?.data.mode === "independent" && !burst.data.published && (
        <button
          className="secondary"
          onClick={() =>
            op.run(() => command("conversation.finishRound", { id: row.id }))
          }
        >
          End round and show completed opinions
        </button>
      )}
      <h3>Shared Resources</h3>
      {list(rows, "resource")
        .filter((r) => r.data.conversationIds.includes(row.id))
        .map((r) => (
          <button className="related-row" key={r.id} onClick={() => open(r)}>
            <FileText size={15} />
            <span>{r.data.title}</span>
            <ChevronRight size={14} />
          </button>
        ))}
      <h3>Recent responses</h3>
      {reqs
        .slice(-8)
        .reverse()
        .map((r) => (
          <button
            className="related-row"
            key={r.id}
            disabled={!r.data.runId}
            onClick={() => {
              const run = find(rows, r.data.runId);
              if (run) open(run);
            }}
          >
            <span>
              {find(rows, r.data.agentId)?.data.name}
              <small>{r.data.reason}</small>
            </span>
            <Status value={r.data.status} />
          </button>
        ))}
    </>
  );
}

function RunPanel({ row, rows, command }: Props) {
  const [confirmed, setConfirmed] = useState(false),
    op = useOperation(),
    r = row.data;
  return (
    <>
      <p className="eyebrow">
        {r.kind === "task" ? "TASK EXECUTION" : "AGENT RESPONSE"}
      </p>
      <h2>
        {find(rows, r.taskId)?.data.title ||
          find(rows, r.agentId)?.data.name ||
          "Execution record"}
      </h2>
      <Status value={r.status} />
      <p className="muted small">Started {timestamp(row.createdAt)}</p>
      {r.stopRequested && (
        <div className="notice-box">
          Stop was requested. The status below records whether termination was
          observed.
        </div>
      )}
      {r.error && <div className="notice-box warning">{r.error}</div>}
      {r.output && (
        <>
          <h3>Output</h3>
          <div className="document-body">
            <RichText text={r.output} />
          </div>
        </>
      )}
      {r.checkpoint && (
        <details>
          <summary>Code changes</summary>
          {r.checkpoint.commit && (
            <p className="muted small">
              Commit <code>{r.checkpoint.commit}</code>
            </p>
          )}
          {r.checkpoint.branch && (
            <p className="muted small">
              Branch <code>{r.checkpoint.branch}</code>
            </p>
          )}
          <pre>
            {typeof r.checkpoint.diff === "string"
              ? r.checkpoint.diff
              : JSON.stringify(r.checkpoint, null, 2)}
          </pre>
        </details>
      )}
      {r.diff && (
        <details>
          <summary>Patch</summary>
          <pre>
            {typeof r.diff === "string"
              ? r.diff
              : JSON.stringify(r.diff, null, 2)}
          </pre>
        </details>
      )}
      {r.worktree && (
        <>
          <h3>Preserved worktree</h3>
          <code className="path">{r.worktree}</code>
        </>
      )}
      {r.status === "needs_reconciliation" && (
        <form
          className="subform"
          onSubmit={(e) => {
            e.preventDefault();
            op.run(() =>
              command("run.reconcile", {
                id: row.id,
                confirmedStopped: confirmed,
              }),
            );
          }}
        >
          <h3>Reconcile interrupted work</h3>
          <p className="small muted">
            Check the previous runtime before resolving. This preserves its
            output and cancels the uncertain attempt. It does not run the task
            again.
          </p>
          <label className="check-option">
            <input
              type="checkbox"
              checked={confirmed}
              onChange={(e) => setConfirmed(e.target.checked)}
            />
            <span>I checked that the previous runtime has stopped.</span>
          </label>
          <ErrorNote error={op.error} />
          <Submit pending={op.pending} disabled={!confirmed || op.pending}>
            Mark previous attempt stopped
          </Submit>
        </form>
      )}
      <h3>Activity</h3>
      <ol className="event-timeline">
        {(r.events || []).map((event: any, i: number) => (
          <li key={i}>
            <span className="timeline-dot" />
            <div>
              <strong>{label(event.type)}</strong>
              <small>{timestamp(event.at)}</small>
              {Object.keys(event.data || {}).length > 0 && (
                <details>
                  <summary>Details</summary>
                  <pre>{JSON.stringify(event.data, null, 2)}</pre>
                </details>
              )}
            </div>
          </li>
        ))}
      </ol>
      <details className="technical-details">
        <summary>Context sent to the agent</summary>
        <p className="muted small">
          This is the recorded input. Later changes were not part of that
          initial context.
        </p>
        <pre>{r.packet}</pre>
        <h4>Manifest</h4>
        <pre>{JSON.stringify(r.manifest, null, 2)}</pre>
        <h4>Input digest</h4>
        <code>{r.digest}</code>
      </details>
      <details className="technical-details">
        <summary>Runtime receipt</summary>
        <pre>{JSON.stringify(r.runtime || {}, null, 2)}</pre>
      </details>
    </>
  );
}

function TaskForm({ rows, task, command, onDone, projectId }: Props) {
  const original = task?.data || {},
    [title, setTitle] = useState(original.title || ""),
    [description, setDescription] = useState(original.description || ""),
    [criteria, setCriteria] = useState((original.criteria || []).join("\n")),
    [project, setProject] = useState(original.projectId || projectId || ""),
    [parent, setParent] = useState(original.parentId || ""),
    [kind, setKind] = useState(original.kind || "feature"),
    [assignee, setAssignee] = useState(original.assigneeId || ""),
    [execution, setExecution] = useState(original.execution || "chat"),
    [parallel, setParallel] = useState(original.parallel !== false),
    [selected, setSelected] = useState(original.selected !== false),
    [blocker, setBlocker] = useState(original.blocker || ""),
    [locks, setLocks] = useState((original.locks || []).join("\n")),
    [deps, setDeps] = useState<string[]>(original.dependencies || []),
    [resources, setResources] = useState<Attachment[]>(
      original.resources || [],
    ),
    [status, setStatus] = useState(original.status || "draft"),
    [advanced, setAdvanced] = useState(!!task),
    [base] = useState(task?.revision),
    op = useOperation();
  const candidates = list(rows, "task").filter((t) => t.id !== task?.id);
  return (
    <form
      className="task-form"
      onSubmit={(e) => {
        e.preventDefault();
        op.run(async () => {
          const result = await command(task ? "task.update" : "task.create", {
            ...(task ? { id: task.id, expectedRevision: base, status } : {}),
            title,
            description,
            criteria: lines(criteria),
            projectId: project || null,
            assigneeId: assignee || null,
            parentId: parent || null,
            kind,
            execution,
            parallel,
            selected,
            blocker,
            locks: lines(locks),
            dependencies: deps,
            resources,
          });
          onDone(result);
        });
      }}
    >
      <Field label="Task title">
        <input
          value={title}
          onChange={(e) => setTitle(e.target.value)}
          placeholder="What needs to happen?"
          required
          maxLength={240}
          autoFocus
        />
      </Field>
      <Field
        label="Outcome"
        hint="Describe what should be true when this is done."
      >
        <textarea
          value={description}
          onChange={(e) => setDescription(e.target.value)}
          placeholder="A clear result the agent can work toward…"
          rows={4}
        />
      </Field>
      <Field
        label="Acceptance criteria"
        hint="One criterion per line. These are required before execution."
      >
        <textarea
          value={criteria}
          onChange={(e) => setCriteria(e.target.value)}
          placeholder={"The new behavior works\nRelevant checks pass"}
          rows={3}
        />
      </Field>
      <div className="form-grid">
        <Field label="Project">
          <select value={project} onChange={(e) => setProject(e.target.value)}>
            <option value="">Loose task</option>
            {list(rows, "project").map((p) => (
              <option value={p.id} key={p.id}>
                {p.data.name}
              </option>
            ))}
          </select>
        </Field>
        <Field label="Type">
          <select value={kind} onChange={(e) => setKind(e.target.value)}>
            {["feature", "bug", "research", "chore"].map((k) => (
              <option key={k}>{k}</option>
            ))}
          </select>
        </Field>
      </div>
      <Field
        label="Assigned agent"
        hint="Optional. Choose the agent whose skills fit this task."
      >
        <select value={assignee} onChange={(e) => setAssignee(e.target.value)}>
          <option value="">Choose when work starts</option>
          {list(rows, "agent")
            .filter((a) => a.data.enabled)
            .map((a) => (
              <option key={a.id} value={a.id}>
                {a.data.name}
              </option>
            ))}
        </select>
      </Field>
      {assignee &&
        (find(rows, assignee)?.data.deliveryMode || "managed") !==
          "managed" && (
          <p className="audience-notice">
            <Users size={14} />
            <span>
              {execution === "code"
                ? "Choose a local agent for repository execution."
                : `Mark this task ready to place it in ${find(rows, assignee)?.data.name}'s inbox. It will work within its own connected runtime.`}
            </span>
          </p>
        )}
      <button
        className="disclosure"
        type="button"
        onClick={() => setAdvanced(!advanced)}
      >
        {advanced ? <ChevronDown size={15} /> : <ChevronRight size={15} />}{" "}
        Dependencies & execution
      </button>
      {advanced && (
        <div className="advanced-fields">
          <div className="form-grid">
            <Field label="Parent task">
              <select
                value={parent}
                onChange={(e) => setParent(e.target.value)}
              >
                <option value="">Top level</option>
                {candidates
                  .filter((t) => t.data.projectId === project)
                  .map((t) => (
                    <option key={t.id} value={t.id}>
                      {t.data.title}
                    </option>
                  ))}
              </select>
            </Field>
            <Field label="Execution">
              <select
                value={execution}
                onChange={(e) => setExecution(e.target.value)}
              >
                <option value="chat">Agent research / writing</option>
                <option value="code">Code in isolated worktree</option>
              </select>
            </Field>
          </div>
          {task && (
            <Field label="Status">
              <select
                value={status}
                onChange={(e) => setStatus(e.target.value)}
              >
                {[
                  "draft",
                  "ready",
                  "review",
                  "cancelled",
                  ...(original.status === "accepted" ? ["accepted"] : []),
                ].map((s) => (
                  <option key={s} value={s}>
                    {label(s)}
                  </option>
                ))}
              </select>
            </Field>
          )}
          <Checks
            title="Dependencies"
            items={candidates.map((t) => ({
              id: t.id,
              title: t.data.title,
              detail: label(t.data.status),
            }))}
            selected={deps}
            setSelected={setDeps}
          />
          <Field label="Blocker">
            <input
              value={blocker}
              onChange={(e) => setBlocker(e.target.value)}
              placeholder="Leave empty when unblocked"
            />
          </Field>
          <label className="check-option">
            <input
              type="checkbox"
              checked={selected}
              onChange={(e) => setSelected(e.target.checked)}
            />
            <span>
              This alternative is selected
              <small>Unselected alternatives cannot execute.</small>
            </span>
          </label>
          <label className="check-option">
            <input
              type="checkbox"
              checked={parallel}
              onChange={(e) => setParallel(e.target.checked)}
            />
            <span>
              May run alongside other tasks
              <small>Dependencies and shared locks still apply.</small>
            </span>
          </label>
          <Field
            label="Shared locks"
            hint="One name per line, such as schema or release-config. Tasks sharing a lock run one at a time."
          >
            <textarea
              value={locks}
              onChange={(e) => setLocks(e.target.value)}
              rows={2}
            />
          </Field>
          <Checks
            title="Pin Resource revisions"
            items={list(rows, "resource").map((r) => ({
              id: r.id,
              title: r.data.title,
              detail: `Revision ${find(rows, resources.find((p) => p.resourceId === r.id)?.revisionId || r.data.latestRevisionId)?.data.number}`,
            }))}
            selected={resources.map((p) => p.resourceId)}
            setSelected={(ids) =>
              setResources(
                ids.map(
                  (id) =>
                    resources.find((p) => p.resourceId === id) || {
                      resourceId: id,
                      revisionId: find(rows, id)!.data.latestRevisionId,
                    },
                ),
              )
            }
          />
        </div>
      )}
      <ErrorNote error={op.error} />
      {task && base !== task.revision && (
        <p className="notice-box">
          This task changed elsewhere. Your draft is kept here; reopen the task
          to load the current version before saving.
        </p>
      )}
      <div className="form-actions">
        <Submit pending={op.pending}>
          {task ? "Save task" : "Create task"}
        </Submit>
      </div>
    </form>
  );
}

export function ModalContent(props: Props) {
  const {
    modal,
    rows,
    command,
    done,
    close,
    setPanel,
    state,
    auth,
    fetcher,
    refresh,
    setAuth,
    setNotice,
    onNavigate,
  } = props;
  if (modal.type === "agent")
    return <AgentForm key={modal.edit?.id || "new"} {...props} />;
  if (modal.type === "resource-attach")
    return <ResourceAttachmentForm {...props} />;
  if (modal.type === "resource-organize")
    return <ResourceOrganizationForm {...props} />;
  if (modal.type === "space-existing") return <SpaceExistingForm {...props} />;
  if (modal.type === "task")
    return (
      <TaskForm
        rows={rows}
        projectId={modal.projectId}
        command={command}
        onDone={(row: Row) => {
          close();
          setPanel(row);
        }}
      />
    );
  if (modal.type === "settings") return <SettingsForm {...props} />;
  if (modal.type === "scope") return <ScopeForm {...props} />;
  if (modal.type === "resource") return <NewResource {...props} />;
  if (modal.type === "forward") return <ForwardForm {...props} />;
  if (modal.type === "link") return <GithubForm {...props} />;
  if (modal.type === "plan") return <PlanForm {...props} />;
  if (modal.type === "conversation") return <ConversationForm {...props} />;
  return <PlaceForm {...props} />;
}

function PlaceForm({ modal, rows, command, close, setNotice }: Props) {
  const original = modal.edit?.data || {},
    [name, setName] = useState(original.name || ""),
    [description, setDescription] = useState(original.description || ""),
    [space, setSpace] = useState(original.spaceId || modal.spaceId || ""),
    [repository, setRepository] = useState(original.repository || ""),
    [targetBranch, setBranch] = useState(original.targetBranch || "main"),
    [github, setGithub] = useState(original.github || ""),
    [color, setColor] = useState(original.color || "#527868"),
    [agentIds, setAgentIds] = useState<string[]>(
      original.agentIds || find(rows, modal.spaceId)?.data.agentIds || [],
    ),
    op = useOperation();
  return (
    <form
      onSubmit={(e) => {
        e.preventDefault();
        op.run(async () => {
          await command(`${modal.type}.${modal.edit ? "update" : "create"}`, {
            ...(modal.edit
              ? { id: modal.edit.id, expectedRevision: modal.edit.revision }
              : {}),
            name,
            description,
            agentIds,
            spaceId: space || null,
            repository: repository || null,
            targetBranch,
            github: github || null,
            color,
          });
          close();
          setNotice(
            modal.edit
              ? `${modal.type === "space" ? "Space" : "Project"} updated.${modal.type === "project" ? " Existing approvals need a fresh review." : ""}`
              : `${modal.type === "space" ? "Space" : "Project"} created.`,
          );
        });
      }}
    >
      <Field label="Name">
        <input
          value={name}
          onChange={(e) => setName(e.target.value)}
          required
          maxLength={120}
          autoFocus
          placeholder={
            modal.type === "space"
              ? "A corner of your world"
              : "Something you want to make"
          }
        />
      </Field>
      <Field label={modal.type === "space" ? "Description" : "Project outcome"}>
        <textarea
          value={description}
          onChange={(e) => setDescription(e.target.value)}
          rows={4}
          placeholder="What belongs here?"
        />
      </Field>
      <Checks
        title={`Agents in this ${modal.type}`}
        items={list(rows, "agent")
          .filter((a) => a.data.enabled)
          .map((a) => ({ id: a.id, title: a.data.name, detail: a.data.role }))}
        selected={agentIds}
        setSelected={setAgentIds}
      />
      <p className="muted small">
        These agents can participate here. Individual Resources keep their own
        sharing settings.
      </p>
      {modal.type === "space" ? (
        <Field label="Color">
          <input
            type="color"
            value={color}
            onChange={(e) => setColor(e.target.value)}
          />
        </Field>
      ) : (
        <>
          <Field label="Space">
            <select value={space} onChange={(e) => setSpace(e.target.value)}>
              <option value="">No space</option>
              {list(rows, "space").map((s) => (
                <option key={s.id} value={s.id}>
                  {s.data.name}
                </option>
              ))}
            </select>
          </Field>
          <details className="form-details" open={!!original.repository}>
            <summary>Code project settings</summary>
            <p className="muted small">
              Only add repositories you own and authorize this workspace to use.
              The app preserves work in separate worktrees.
            </p>
            <Field label="Local repository path">
              <input
                value={repository}
                onChange={(e) => setRepository(e.target.value)}
                placeholder="/absolute/path/to/repository"
              />
            </Field>
            <Field label="Target branch">
              <input
                value={targetBranch}
                onChange={(e) => setBranch(e.target.value)}
                placeholder="main"
              />
            </Field>
            <Field
              label="GitHub repository"
              hint="Optional, for linked pull requests. Publishing is not enabled."
            >
              <input
                value={github}
                onChange={(e) => setGithub(e.target.value)}
                placeholder="owner/repository"
              />
            </Field>
          </details>
        </>
      )}
      <ErrorNote error={op.error} />
      <div className="form-actions">
        <button type="button" className="secondary" onClick={close}>
          Cancel
        </button>
        <Submit pending={op.pending}>
          {modal.edit ? "Save changes" : `Create ${modal.type}`}
        </Submit>
      </div>
    </form>
  );
}

function ConversationForm({ modal, rows, command, close, onNavigate }: Props) {
  const [title, setTitle] = useState(""),
    [agents, setAgents] = useState<string[]>(
      find(rows, modal.spaceId)?.data.agentIds || [],
    ),
    [kind, setKind] = useState(modal.kind || "conversation"),
    [space, setSpace] = useState(modal.spaceId || ""),
    [project, setProject] = useState(modal.projectId || ""),
    op = useOperation();
  return (
    <form
      onSubmit={(e) => {
        e.preventDefault();
        op.run(async () => {
          const c = await command("conversation.create", {
            title:
              title.trim() ||
              agents.map((id) => find(rows, id)?.data.name).join(" & "),
            agentIds: agents,
            kind: kind === "channel" ? "channel" : undefined,
            spaceId: space || undefined,
            projectId: project || undefined,
          });
          close();
          onNavigate?.("Messages", c.id);
        });
      }}
    >
      <div className="segmented">
        <button
          type="button"
          className={kind === "conversation" ? "active" : ""}
          onClick={() => setKind("conversation")}
        >
          <MessageSquare size={14} /> Conversation
        </button>
        <button
          type="button"
          className={kind === "channel" ? "active" : ""}
          onClick={() => setKind("channel")}
        >
          <Users size={14} /> Channel
        </button>
      </div>
      <Field
        label="Name"
        hint="Optional. We'll use the participants' names if you leave this empty."
      >
        <input
          value={title}
          onChange={(e) => setTitle(e.target.value)}
          autoFocus
          placeholder="A name for this conversation"
          maxLength={120}
        />
      </Field>
      <Checks
        title="Invite agents"
        items={list(rows, "agent")
          .filter((a) => a.data.enabled)
          .map((a) => ({ id: a.id, title: a.data.name, detail: a.data.role }))}
        selected={agents}
        setSelected={setAgents}
      />
      <Field label="Space">
        <select
          value={space}
          onChange={(e) => {
            setSpace(e.target.value);
            if (!agents.length)
              setAgents(find(rows, e.target.value)?.data.agentIds || []);
          }}
        >
          <option value="">No space</option>
          {list(rows, "space").map((s) => (
            <option key={s.id} value={s.id}>
              {s.data.name}
            </option>
          ))}
        </select>
      </Field>
      <Field label="Project context">
        <select value={project} onChange={(e) => setProject(e.target.value)}>
          <option value="">No project</option>
          {list(rows, "project").map((p) => (
            <option key={p.id} value={p.id}>
              {p.data.name}
            </option>
          ))}
        </select>
      </Field>
      <p className="muted small">
        Resources are shared separately. A new conversation starts with a fresh
        history.
      </p>
      <ErrorNote error={op.error} />
      <div className="form-actions">
        <Submit pending={op.pending} disabled={!agents.length || op.pending}>
          {kind === "channel" ? "Create channel" : "Start conversation"}{" "}
          <ArrowRight size={15} />
        </Submit>
      </div>
    </form>
  );
}

function NewResource({
  modal,
  rows,
  command,
  close,
  auth,
  refresh,
  setPanel,
}: Props) {
  const [kind, setKind] = useState("note"),
    [title, setTitle] = useState(""),
    [text, setText] = useState(modal.text || ""),
    [file, setFile] = useState<File | null>(null),
    [conversations, setConversations] = useState<string[]>(
      modal.conversationId ? [modal.conversationId] : [],
    ),
    [projects, setProjects] = useState<string[]>(
      modal.projectId ? [modal.projectId] : [],
    ),
    [spaces, setSpaces] = useState<string[]>(
      modal.spaceId ? [modal.spaceId] : [],
    ),
    [tags, setTags] = useState(""),
    [pinned, setPinned] = useState(false),
    op = useOperation();
  const audience = [
    ...new Set<string>([
      ...conversations.flatMap((id) => find(rows, id)?.data.agentIds || []),
      ...spaces.flatMap((id) => find(rows, id)?.data.agentIds || []),
    ]),
  ];
  return (
    <form
      onSubmit={(e) => {
        e.preventDefault();
        op.run(async () => {
          let result;
          if (kind === "file") {
            if (!file) throw new Error("Choose a file to upload.");
            if (file.size > 10 * 1024 * 1024)
              throw new Error("Choose a file smaller than 10 MB.");
            const form = new FormData();
            form.append("title", title || file.name);
            form.append("conversationIds", JSON.stringify(conversations));
            form.append("projectIds", JSON.stringify(projects));
            form.append("spaceIds", JSON.stringify(spaces));
            form.append("agentIds", JSON.stringify(audience));
            form.append(
              "tags",
              JSON.stringify(
                tags
                  .split(",")
                  .map((v) => v.trim())
                  .filter(Boolean),
              ),
            );
            form.append("pinned", String(pinned));
            form.append("file", file);
            const response = await fetch("/api/uploads", {
              method: "POST",
              headers: { "X-CSRF-Token": auth.csrf },
              body: form,
            });
            result = await response.json();
            if (!response.ok) throw new Error(result.error || "Upload failed.");
            await refresh();
          } else
            result = await command("resource.create", {
              title,
              type: kind,
              text,
              conversationIds: conversations,
              projectIds: projects,
              spaceIds: spaces,
              agentIds: audience,
              pinned,
              tags: tags
                .split(",")
                .map((v) => v.trim())
                .filter(Boolean),
            });
          close();
          setPanel(result.resource || result.value?.resource || result);
        });
      }}
    >
      <div className="segmented">
        {["note", "paste", "file"].map((k) => (
          <button
            key={k}
            type="button"
            className={kind === k ? "active" : ""}
            onClick={() => setKind(k)}
          >
            {k === "file" ? <Upload size={15} /> : <FileText size={15} />}{" "}
            {k === "note" ? "Note" : k === "paste" ? "Paste" : "File"}
          </button>
        ))}
      </div>
      <Field label="Title">
        <input
          value={title}
          onChange={(e) => setTitle(e.target.value)}
          required={kind !== "file"}
          maxLength={240}
          autoFocus
          placeholder="Make this easy to find later"
        />
      </Field>
      {kind === "file" ? (
        <Field label="File">
          <div className="upload-zone">
            <Upload size={25} />
            <input
              type="file"
              onChange={(e) => {
                const selected = e.target.files?.[0] || null;
                setFile(selected);
                if (!title && selected) setTitle(selected.name);
              }}
            />
            <small>
              {file
                ? `${file.name} · ${fileSize(file.size)}`
                : "Choose a file from this computer · up to 10 MB"}
            </small>
          </div>
        </Field>
      ) : (
        <Field label="Content">
          <textarea
            value={text}
            onChange={(e) => setText(e.target.value)}
            rows={8}
            className={kind === "paste" ? "code-input" : ""}
            placeholder={
              kind === "paste"
                ? "Paste text or code…"
                : "An idea, a useful note, or a document…"
            }
            maxLength={256000}
          />
        </Field>
      )}
      <details
        className="form-details"
        open={!!modal.conversationId || !!modal.projectId || !!modal.spaceId}
      >
        <summary>Share & organize</summary>
        <Checks
          title="Post to spaces"
          items={list(rows, "space").map((s) => ({
            id: s.id,
            title: s.data.name,
          }))}
          selected={spaces}
          setSelected={setSpaces}
        />
        <Field label="Tags">
          <input
            value={tags}
            onChange={(e) => setTags(e.target.value)}
            placeholder="research, design, reference"
          />
        </Field>
        <label className="check-option">
          <input
            type="checkbox"
            checked={pinned}
            onChange={(e) => setPinned(e.target.checked)}
          />
          <span>Pin to the top of its spaces</span>
        </label>
        <Checks
          title="Share with conversations"
          items={list(rows, "conversation").map((c) => ({
            id: c.id,
            title: c.data.title,
          }))}
          selected={conversations}
          setSelected={setConversations}
        />
        <p className="muted small">
          Selected conversations and their current agents receive access.
        </p>
        <Checks
          title="Link to projects"
          items={list(rows, "project").map((p) => ({
            id: p.id,
            title: p.data.name,
          }))}
          selected={projects}
          setSelected={setProjects}
        />
      </details>
      <p className="audience-notice">
        <Users size={14} />
        <span>
          {audience.length
            ? `Shared with ${audience.map((id) => find(rows, id)?.data.name).join(", ")}.`
            : "Private to you until you choose a conversation or shared space."}
        </span>
      </p>
      <ErrorNote error={op.error} />
      <div className="form-actions">
        <Submit pending={op.pending}>
          {kind === "file" ? "Upload Resource" : "Create Resource"}
        </Submit>
      </div>
    </form>
  );
}

function ScopeForm({ modal, rows, command, close, setNotice }: Props) {
  const tasks = list(rows, "task").filter(
      (t) =>
        (!t.data.assigneeId ||
          (find(rows, t.data.assigneeId)?.data.deliveryMode || "managed") ===
            "managed") &&
        (!modal.projectId || t.data.projectId === modal.projectId) &&
        !["accepted", "cancelled", "running", "review"].includes(t.data.status),
    ),
    [selected, setSelected] = useState<string[]>(
      tasks
        .filter(
          (t) =>
            t.data.selected &&
            t.data.description.trim() &&
            t.data.criteria.length,
        )
        .map((t) => t.id),
    ),
    [minutes, setMinutes] = useState(60),
    [workers, setWorkers] = useState(2),
    [title, setTitle] = useState(
      `${find(rows, modal.projectId)?.data.name || "Workspace"} work session`,
    ),
    [ack, setAck] = useState(false),
    [approved, setApproved] = useState<Row | null>(null),
    op = useOperation();
  const preview = JSON.stringify(
    tasks
      .filter((t) => selected.includes(t.id))
      .map((t) => [
        t.id,
        t.revision,
        t.data.projectId,
        find(rows, t.data.projectId)?.revision,
      ]),
  );
  useEffect(() => {
    setAck(false);
  }, [preview, minutes, workers]);
  return (
    <form
      onSubmit={(e) => {
        e.preventDefault();
        op.run(async () => {
          if (!approved) {
            const scope = await command("scope.approve", {
              title,
              taskIds: selected,
              expectedRevisions: Object.fromEntries(
                tasks
                  .filter((t) => selected.includes(t.id))
                  .map((t) => [t.id, t.revision]),
              ),
              expectedProjectRevisions: Object.fromEntries(
                tasks
                  .filter((t) => selected.includes(t.id) && t.data.projectId)
                  .map((t) => [
                    t.data.projectId,
                    find(rows, t.data.projectId)?.revision,
                  ]),
              ),
              maxWorkers: workers,
              minutes,
            });
            setApproved(scope);
          } else {
            await command("session.start", { scopeId: approved.id });
            close();
            setNotice(
              "Work session started. Results will wait for your review.",
            );
          }
        });
      }}
    >
      <p className="muted">
        Choose exactly what the agents may work on. Dependencies, blockers,
        selected alternatives and shared locks still decide what can start.
        Connected-agent assignments use their own inbox instead of a local work
        session.
      </p>
      <Field label="Session name">
        <input
          value={title}
          onChange={(e) => setTitle(e.target.value)}
          required
          disabled={!!approved}
        />
      </Field>
      {approved ? (
        <div className="notice-box success">
          <CheckCircle2 size={18} />
          <strong>Scope approved</strong>
          <p>
            {approved.data.taskIds.length} tasks approved. Starting will check
            that the approved task revisions and project settings are unchanged.
          </p>
        </div>
      ) : null}
      <fieldset disabled={!!approved}>
        <Checks
          title="Included tasks"
          items={tasks.map((t) => ({
            id: t.id,
            title: t.data.title,
            detail: `Revision ${t.revision} · ${t.data.execution === "code" ? "code" : "agent"}${taskReasons(t, rows).length ? ` · ${taskReasons(t, rows)[0]}` : ""}`,
          }))}
          selected={selected}
          setSelected={setSelected}
        />
        <div className="form-grid">
          <Field label="Time limit">
            <select
              value={minutes}
              onChange={(e) => setMinutes(Number(e.target.value))}
            >
              {[5, 15, 30, 60, 90, 120].map((n) => (
                <option key={n} value={n}>
                  {n} minutes
                </option>
              ))}
            </select>
          </Field>
          <Field label="Maximum workers">
            <select
              value={workers}
              onChange={(e) => setWorkers(Number(e.target.value))}
            >
              <option value={1}>1 worker</option>
              <option value={2}>2 workers</option>
            </select>
          </Field>
        </div>
      </fieldset>
      <details className="scope-task-preview">
        <summary>Review selected outcomes and criteria</summary>
        {tasks
          .filter((t) => selected.includes(t.id))
          .map((t) => (
            <article key={t.id}>
              <strong>
                {t.data.title} · revision {t.revision}
              </strong>
              <RichText text={t.data.description || "No outcome yet"} />
              <ul>
                {t.data.criteria.map((c: string, j: number) => (
                  <li key={j}>{c}</li>
                ))}
              </ul>
              {t.data.execution === "code" && (
                <p className="muted small">
                  Repository:{" "}
                  {find(rows, t.data.projectId)?.data.repository ||
                    "Not configured"}{" "}
                  · target{" "}
                  {find(rows, t.data.projectId)?.data.targetBranch || "main"}
                </p>
              )}
            </article>
          ))}
      </details>
      <div className="scope-summary">
        <strong>What this permits</strong>
        <p>
          Work on {selected.length} selected tasks, up to {workers} at once, for
          at most {minutes} minutes. The last 20% of the window is reserved for
          finishing and review. Code tasks use their project's configured
          repository.
        </p>
        <p>
          Results stay local and await review. This scope does not allow
          publishing, merging, deployment, or new paid usage.
        </p>
      </div>
      {approved && (
        <details>
          <summary>Trigger this scope later</summary>
          <p className="muted small">
            This exact scope can be started from its project's Activity tab or
            the local command below. Changes to its tasks require a new
            approval.
          </p>
          <code className="path">pnpm trigger {approved.id}</code>
        </details>
      )}
      {!approved && (
        <label className="check-option">
          <input
            type="checkbox"
            checked={ack}
            onChange={(e) => setAck(e.target.checked)}
          />
          <span>I approve these task revisions and the limits above.</span>
        </label>
      )}
      <ErrorNote error={op.error} />
      <div className="form-actions">
        <button type="button" className="secondary" onClick={close}>
          {approved ? "Keep approved scope for later" : "Cancel"}
        </button>
        <Submit
          pending={op.pending}
          disabled={op.pending || (!approved && (!selected.length || !ack))}
        >
          {approved ? (
            <>
              <Play size={15} /> Start approved session
            </>
          ) : (
            "Approve scope"
          )}
        </Submit>
      </div>
    </form>
  );
}

function PlanForm({ modal, rows, command, close, onNavigate }: Props) {
  const [request, setRequest] = useState(
      `Help me plan ${modal.project.data.name}. ${modal.project.data.description || ""}`,
    ),
    [agent, setAgent] = useState(list(rows, "agent")[0]?.id || ""),
    op = useOperation();
  return (
    <form
      onSubmit={(e) => {
        e.preventDefault();
        op.run(async () => {
          const c = await command("conversation.create", {
            title: `Planning · ${modal.project.data.name}`.slice(0, 120),
            agentIds: [agent],
            projectId: modal.project.id,
          });
          await command("message.send", {
            conversationId: c.id,
            body: request,
            intent: "planning",
          });
          close();
          onNavigate?.("Messages", c.id);
        });
      }}
    >
      <p className="muted">
        Start a project conversation. The agent can inspect existing tasks and
        suggest a plan for you to review.
      </p>
      <Field label="What do you want to plan?">
        <textarea
          value={request}
          onChange={(e) => setRequest(e.target.value)}
          rows={6}
          required
          autoFocus
        />
      </Field>
      <Field label="Agent">
        <select value={agent} onChange={(e) => setAgent(e.target.value)}>
          {list(rows, "agent")
            .filter((a) => a.data.enabled)
            .map((a) => (
              <option key={a.id} value={a.id}>
                {a.data.name}
              </option>
            ))}
        </select>
      </Field>
      <ErrorNote error={op.error} />
      <div className="form-actions">
        <Submit pending={op.pending}>
          <MessageSquare size={15} /> Open planning conversation
        </Submit>
      </div>
    </form>
  );
}

function ForwardForm({ modal, rows, command, close }: Props) {
  const [conversation, setConversation] = useState(""),
    [note, setNote] = useState(""),
    op = useOperation();
  const source = modal.message;
  return (
    <form
      onSubmit={(e) => {
        e.preventDefault();
        op.run(async () => {
          await command("message.send", {
            conversationId: conversation,
            body: [
              note.trim(),
              ...source.data.body.split("\n").map((s: string) => `> ${s}`),
            ]
              .filter(Boolean)
              .join("\n"),
            forwardedFrom: source.id,
            attachments: [],
          });
          close();
        });
      }}
    >
      <Field label="To conversation">
        <select
          value={conversation}
          onChange={(e) => setConversation(e.target.value)}
          required
        >
          <option value="">Choose a conversation</option>
          {list(rows, "conversation")
            .filter((c) => c.id !== source.data.conversationId)
            .map((c) => (
              <option key={c.id} value={c.id}>
                {c.data.title}
              </option>
            ))}
        </select>
      </Field>
      <Field label="Add a message">
        <textarea
          value={note}
          onChange={(e) => setNote(e.target.value)}
          placeholder="What would you like them to do with this?"
          rows={3}
        />
      </Field>
      <blockquote className="forward-preview">{source.data.body}</blockquote>
      <p className="muted small">
        The forwarded text is quoted. Attachments and access to the original
        conversation are not shared.
      </p>
      <ErrorNote error={op.error} />
      <div className="form-actions">
        <Submit pending={op.pending} disabled={!conversation || op.pending}>
          Forward message <ArrowRight size={15} />
        </Submit>
      </div>
    </form>
  );
}

function PullLink({ row }: { row: Row }) {
  const data = row.data;
  return (
    <a className="pull-link" href={data.url} target="_blank" rel="noreferrer">
      <GitBranch size={18} />
      <span>
        <strong>{data.title || data.pull?.title || "Pull request"}</strong>
        <small>
          {data.state || data.pull?.state || "Linked"} · {data.url}
        </small>
      </span>
      <ExternalLink size={14} />
    </a>
  );
}
function GithubForm({
  modal,
  rows,
  command,
  fetcher,
  close,
  setNotice,
}: Props) {
  const [url, setUrl] = useState(""),
    [pull, setPull] = useState<any>(null),
    [task, setTask] = useState(modal.taskId || ""),
    [project, setProject] = useState(modal.projectId || ""),
    op = useOperation();
  return (
    <form
      onSubmit={(e) => {
        e.preventDefault();
        op.run(async () => {
          if (!pull) {
            setPull(
              await fetcher(`/api/github/pull?url=${encodeURIComponent(url)}`),
            );
          } else {
            await command("github.link", {
              url,
              taskId: task || undefined,
              projectId: project || undefined,
            });
            close();
            setNotice("Pull request linked.");
          }
        });
      }}
    >
      <p className="muted">
        Link an existing pull request. This reads GitHub using the account
        already signed in on this Mac.
      </p>
      <Field label="Pull request URL">
        <input
          type="url"
          required
          value={url}
          onChange={(e) => {
            setUrl(e.target.value);
            setPull(null);
          }}
          placeholder="https://github.com/owner/repo/pull/123"
          autoFocus
        />
      </Field>
      <Field label="Project">
        <select value={project} onChange={(e) => setProject(e.target.value)}>
          <option value="">No project</option>
          {list(rows, "project").map((p) => (
            <option key={p.id} value={p.id}>
              {p.data.name}
            </option>
          ))}
        </select>
      </Field>
      <Field label="Task">
        <select value={task} onChange={(e) => setTask(e.target.value)}>
          <option value="">No task</option>
          {list(rows, "task")
            .filter((t) => !project || t.data.projectId === project)
            .map((t) => (
              <option key={t.id} value={t.id}>
                {t.data.title}
              </option>
            ))}
        </select>
      </Field>
      {pull && (
        <div className="notice-box success">
          <strong>
            {pull.title || pull.pull?.title || "Pull request found"}
          </strong>
          <p>
            {pull.state || pull.pull?.state || "Available"}
            {pull.draft ? " · Draft" : ""}
          </p>
          <details>
            <summary>GitHub details</summary>
            <pre>{JSON.stringify(pull, null, 2)}</pre>
          </details>
        </div>
      )}
      <ErrorNote error={op.error} />
      <div className="form-actions">
        <Submit pending={op.pending}>
          {pull ? "Link pull request" : "Check pull request"}
        </Submit>
      </div>
    </form>
  );
}

function SettingsForm({
  state,
  command,
  auth,
  fetcher,
  setAuth,
  close,
}: Props) {
  const op = useOperation(),
    runtime = state.runtime || {};
  return (
    <>
      <div className="setting-row">
        <span>
          <strong>Agent dispatch</strong>
          <small>
            Pause new responses and task admissions. Saved work stays available.
          </small>
        </span>
        <button
          className={`toggle ${state.dispatchEnabled ? "on" : ""}`}
          aria-label="Agent dispatch"
          aria-pressed={state.dispatchEnabled}
          disabled={op.pending}
          onClick={() =>
            op.run(() =>
              command("settings.dispatch", { enabled: !state.dispatchEnabled }),
            )
          }
        >
          <span />
        </button>
      </div>
      <div className="setting-row">
        <span>
          <strong>Background worker</strong>
          <small>
            {state.workerOnline
              ? "Connected and checking for approved work."
              : "Offline. Messages remain queued until the worker returns."}
          </small>
        </span>
        <Status value={state.workerOnline ? "active" : "offline"} />
      </div>
      <QuotaInfo quota={state.quota} />
      {state.storage && !state.storage.ok && (
        <div className="notice-box warning">
          Stored files need attention. {state.storage.missing?.length || 0}{" "}
          missing, {state.storage.mismatched?.length || 0} with changed
          checksums. Open the local health report before using affected files.
        </div>
      )}
      <WorkQueue rows={state.records} command={command} />
      <h3>Connections</h3>
      <div className="connection-card">
        <span className="avatar">C</span>
        <div>
          <strong>Local Codex</strong>
          <small>
            {runtime.available === false
              ? "Unavailable"
              : runtime.qualified === false
                ? "Qualification pending"
                : "Uses your existing local Codex sign-in"}
          </small>
        </div>
      </div>
      <div className="connection-card">
        <GitBranch size={22} />
        <div>
          <strong>GitHub</strong>
          <small>Existing account for reads. Publishing is not enabled.</small>
        </div>
      </div>
      <p className="muted small">
        Connected agents receive only work and material shared with their
        identities.
      </p>
      <details className="technical-details">
        <summary>Connection details</summary>
        <pre>
          {JSON.stringify(
            {
              runtime: state.runtime,
              github: state.github,
              quota: state.quota,
              storage: state.storage,
            },
            null,
            2,
          )}
        </pre>
      </details>
      <ErrorNote error={op.error} />
      <div className="form-actions">
        <button
          className="secondary"
          disabled={op.pending}
          onClick={() =>
            op.run(async () => {
              await fetcher("/api/logout", {
                method: "POST",
                headers: {
                  "Content-Type": "application/json",
                  "X-CSRF-Token": auth.csrf,
                },
                body: "{}",
              });
              setAuth({ authenticated: false });
              close();
            })
          }
        >
          Sign out
        </button>
        <button className="primary" onClick={close}>
          Done
        </button>
      </div>
    </>
  );
}

function AgentForm({
  modal,
  rows,
  command,
  close,
  setNotice,
  updateModal,
}: Props) {
  const agent = modal.edit
      ? find(rows, modal.edit.id) || modal.edit
      : undefined,
    [tab, setTab] = useState(modal.tab || "identity"),
    [identityRevision] = useState(agent?.revision),
    [deliveryMode, setDeliveryMode] = useState("managed"),
    [name, setName] = useState(agent?.data.name || ""),
    [role, setRole] = useState(agent?.data.role || ""),
    [capabilities, setCapabilities] = useState(
      (agent?.data.capabilities || []).join(", "),
    ),
    [enabled, setEnabled] = useState(agent?.data.enabled !== false),
    [color, setColor] = useState(agent?.data.color || "#527868"),
    op = useOperation();
  return (
    <>
      {agent && (
        <div className="agent-tabs tabs">
          <button
            className={tab === "identity" ? "active" : ""}
            onClick={() => setTab("identity")}
          >
            Identity
          </button>
          <button
            className={tab === "connection" ? "active" : ""}
            onClick={() => setTab("connection")}
          >
            Connection & access
          </button>
        </div>
      )}
      {agent && tab === "connection" ? (
        <AgentConnection row={agent} command={command} setNotice={setNotice} />
      ) : (
        <form
          onSubmit={(e) => {
            e.preventDefault();
            op.run(async () => {
              let result = await command(
                agent ? "agent.update" : "agent.create",
                {
                  ...(agent
                    ? { id: agent.id, expectedRevision: identityRevision }
                    : {}),
                  name,
                  role,
                  capabilities: capabilities
                    .split(",")
                    .map((v: string) => v.trim())
                    .filter(Boolean),
                  enabled,
                  color,
                },
              );
              if (!agent && deliveryMode !== "managed") {
                result = await command("agent.configure", {
                  id: result.id,
                  expectedRevision: result.revision,
                  deliveryMode,
                  checkInMinutes: 5,
                });
                updateModal({ type: "agent", edit: result, tab: "connection" });
              } else close();
              setNotice(
                agent
                  ? "Agent identity updated."
                  : "Agent created. Start a conversation to work together.",
              );
            });
          }}
        >
          <p className="muted">
            Give an agent a useful perspective. Choose local Codex or connect an
            agent with its own runtime.
          </p>
          <Field
            label="Name"
            hint={
              agent
                ? "Renaming keeps this agent’s conversations and access."
                : "Letters, numbers, spaces and hyphens. Start with a letter."
            }
          >
            <input
              value={name}
              onChange={(e) => setName(e.target.value)}
              required
              maxLength={40}
              autoFocus
              pattern="[A-Za-z][A-Za-z0-9 _-]*"
              placeholder="A name for your collaborator"
            />
          </Field>
          <Field label="Role and perspective">
            <textarea
              value={role}
              onChange={(e) => setRole(e.target.value)}
              required
              maxLength={2000}
              rows={4}
              placeholder="What should this agent focus on? How should it approach its work?"
            />
          </Field>
          <Field
            label="Capabilities"
            hint="Comma-separated topics help Auto choose the right person, such as research, writing, design."
          >
            <input
              value={capabilities}
              onChange={(e) => setCapabilities(e.target.value)}
              placeholder="research, planning, design"
            />
          </Field>
          {!agent && (
            <Field label="Connection">
              <select
                value={deliveryMode}
                onChange={(e) => setDeliveryMode(e.target.value)}
              >
                <option value="managed">
                  Local Codex · ready on this host
                </option>
                <option value="poll">Connect my own agent</option>
              </select>
              <small>
                Connected agents get an access token and check their inbox.
                Webhook setup is available after creation.
              </small>
            </Field>
          )}
          <Field label="Identity color">
            <input
              type="color"
              value={color}
              onChange={(e) => setColor(e.target.value)}
            />
          </Field>
          {agent && (
            <label className="check-option">
              <input
                type="checkbox"
                checked={enabled}
                onChange={(e) => setEnabled(e.target.checked)}
              />
              <span>
                Available for new responses
                <small>
                  Disabling this identity also requests a stop for active
                  responses.
                </small>
              </span>
            </label>
          )}
          <ErrorNote error={op.error} />
          <div className="form-actions">
            <Submit pending={op.pending}>
              {agent ? "Save identity" : "Create agent"}
            </Submit>
          </div>
        </form>
      )}
    </>
  );
}

function QuotaInfo({ quota }: Props) {
  const buckets = Array.isArray(quota?.buckets) ? quota.buckets : [];
  const duration = (minutes: number | null) => {
    if (
      typeof minutes !== "number" ||
      !Number.isFinite(minutes) ||
      minutes <= 0
    )
      return null;
    if (minutes % 1440 === 0) return `${minutes / 1440} day`;
    if (minutes % 60 === 0) return `${minutes / 60} hour`;
    return `${minutes} minute`;
  };
  return (
    <div className="quota-info">
      <h3>Usage allowance</h3>
      {quota?.admission === "hold" ? (
        <div className="notice-box warning">
          <strong>New work is held</strong>
          <p>
            {quota.reason || "The runtime has not confirmed available usage."}
          </p>
        </div>
      ) : !quota?.known || quota?.status === "unknown" ? (
        <p className="muted small">
          Remaining usage is unknown.{" "}
          {quota?.reason || "Usage telemetry is unavailable."} Sessions still
          obey their time and worker limits.
        </p>
      ) : (
        <p className="muted small">
          {quota.reason || "The runtime reports remaining capacity."}
        </p>
      )}
      {quota?.observedAt && (
        <p className="muted small">
          Last observed {timestamp(quota.observedAt)}
          {quota.fresh === false ? " · report is stale" : ""}. Reset times are
          reported by the runtime.
        </p>
      )}
      {buckets.map((bucket: any) => (
        <div key={bucket.id}>
          {(Array.isArray(bucket.windows) ? bucket.windows : []).map(
            (window: any) => {
              const valid =
                window.validity === "valid" &&
                typeof window.remainingPercent === "number" &&
                Number.isFinite(window.remainingPercent) &&
                window.remainingPercent >= 0 &&
                window.remainingPercent <= 100;
              return (
                <div className="quota-window" key={window.kind}>
                  <span>
                    {bucket.name || bucket.id} ·{" "}
                    {duration(window.durationMinutes) || window.kind} window
                  </span>
                  <strong>
                    {valid
                      ? `${window.remainingPercent}% ${quota.fresh === false ? "last reported remaining" : "remaining"}`
                      : "Unknown"}
                  </strong>
                  {window.resetsAt &&
                    Number.isFinite(Date.parse(window.resetsAt)) && (
                      <small>Reported reset {timestamp(window.resetsAt)}</small>
                    )}
                </div>
              );
            },
          )}
        </div>
      ))}
    </div>
  );
}

function WorkQueue({ rows, command }: Props) {
  const op = useOperation(),
    sessions = list(rows, "workSession"),
    active = sessions.filter((s) =>
      ["active", "stopping", "needs_reconciliation"].includes(s.data.status),
    ),
    unused = list(rows, "scope").filter(
      (s) => !sessions.some((session) => session.data.scopeId === s.id),
    );
  if (!active.length && !unused.length) return null;
  return (
    <div className="work-queue">
      <h3>Work sessions</h3>
      {active.map((s) => (
        <div className="session-card" key={s.id}>
          <header>
            <strong>
              {find(rows, s.data.scopeId)?.data.title || "Work session"}
            </strong>
            <Status value={s.data.status} />
          </header>
          <p>
            {s.data.taskIds.length} tasks · ends {timestamp(s.data.deadline)}
          </p>
          {s.data.status === "active" && (
            <button
              className="secondary"
              disabled={op.pending}
              onClick={() =>
                op.run(() => command("session.stop", { id: s.id }))
              }
            >
              <Square size={12} /> Stop session
            </button>
          )}
        </div>
      ))}
      {unused.map((s) => (
        <div className="session-card" key={s.id}>
          <header>
            <strong>{s.data.title}</strong>
            <small>Approved scope</small>
          </header>
          <p>
            {s.data.taskIds.length} tasks · up to {s.data.minutes} minutes
          </p>
          <button
            className="secondary"
            disabled={op.pending}
            onClick={() =>
              op.run(() => command("session.start", { scopeId: s.id }))
            }
          >
            <Play size={12} /> Start approved scope
          </button>
          <details>
            <summary>Local trigger</summary>
            <code className="path">pnpm trigger {s.id}</code>
          </details>
        </div>
      ))}
      <ErrorNote error={op.error} />
    </div>
  );
}
