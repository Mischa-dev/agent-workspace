import React, { useEffect, useRef, useState } from "react";
import {
  ArrowLeft,
  ArrowRight,
  Check,
  ChevronRight,
  Clock,
  Copy,
  FileText,
  FolderOpen,
  Hash,
  KeyRound,
  Layers,
  Link2,
  Loader2,
  MessageSquare,
  MoreHorizontal,
  Pin,
  Plus,
  RefreshCw,
  Settings,
  ShieldCheck,
  Trash2,
  Users,
  X,
} from "lucide-react";
import type { RecordRow, Attachment } from "../../../packages/shared/src/index";
type Row = RecordRow<any>;
type Props = Record<string, any>;
const records = (rows: Row[], kind: string) =>
  rows.filter((r) => r.kind === kind);
const find = (rows: Row[], id: string) => rows.find((r) => r.id === id);
const date = (value?: string) =>
  value
    ? new Date(value).toLocaleString(undefined, {
        month: "short",
        day: "numeric",
        hour: "numeric",
        minute: "2-digit",
      })
    : "Not yet";
export function checkInLabel(minutes: number) {
  if (minutes === 10080) return "weekly";
  if (minutes === 1440) return "daily";
  if (minutes === 60) return "every hour";
  if (minutes === 1) return "every minute";
  return `every ${minutes} minutes`;
}

function useAction() {
  const [pending, setPending] = useState(false),
    [error, setError] = useState("");
  const active = useRef(false);
  async function run(fn: () => Promise<any>) {
    if (active.current) return;
    active.current = true;
    setPending(true);
    setError("");
    try {
      return await fn();
    } catch (e: any) {
      setError(
        e.message || "Something went wrong. Your changes are still here.",
      );
    } finally {
      active.current = false;
      setPending(false);
    }
  }
  return { pending, error, run };
}
function ErrorText({ text }: { text: string }) {
  return text ? (
    <div role="alert" className="form-error">
      {text}
    </div>
  ) : null;
}
function State({ status }: { status: string }) {
  return (
    <span className={`status status-${status}`}>
      <span className="status-dot" />
      {status.replaceAll("_", " ")}
    </span>
  );
}
function Empty({ title, children }: Props) {
  return (
    <div className="board-empty">
      <FileText size={24} />
      <h3>{title}</h3>
      <p>{children}</p>
    </div>
  );
}

export function SpacesView({
  rows,
  selected,
  search,
  onNavigate,
  openModal,
  setPanel,
  command,
}: Props) {
  const space = find(rows, selected),
    [filter, setFilter] = useState("all"),
    [tag, setTag] = useState("all"),
    [pinned, setPinned] = useState(false),
    op = useAction();
  useEffect(() => {
    setFilter("all");
    setTag("all");
    setPinned(false);
  }, [selected]);
  if (!space || space.kind !== "space")
    return (
      <div className="page spaces-page">
        <div className="space-page-heading">
          <div>
            <h1>Spaces</h1>
            <p>
              Shared material and conversations, gathered around what matters.
            </p>
          </div>
          <button
            className="primary"
            onClick={() => openModal({ type: "space" })}
          >
            <Plus size={15} /> New space
          </button>
        </div>
        <div className="spaces-index">
          {records(rows, "space")
            .filter((s) =>
              `${s.data.name} ${s.data.description}`
                .toLowerCase()
                .includes(search.toLowerCase()),
            )
            .map((s) => {
              const material = records(rows, "resource").filter((r) =>
                  r.data.spaceIds.includes(s.id),
                ),
                channels = records(rows, "conversation").filter(
                  (c) => c.data.spaceId === s.id && c.data.kind !== "thread",
                );
              return (
                <button
                  className="space-index-row"
                  key={s.id}
                  onClick={() => onNavigate("Spaces", s.id)}
                >
                  <span
                    className="space-marker"
                    style={{ background: s.data.color }}
                  >
                    <Layers size={20} />
                  </span>
                  <span>
                    <strong>{s.data.name}</strong>
                    <p>
                      {s.data.description || "A shared place for your work."}
                    </p>
                    <small>
                      {material.length} Resources · {channels.length}{" "}
                      conversations · {(s.data.agentIds || []).length} agents
                    </small>
                  </span>
                  <ChevronRight size={17} />
                </button>
              );
            })}
          {!records(rows, "space").length && (
            <Empty title="Give your work a shared home">
              Create a space, invite agents, then post notes, files and ideas
              they can return to.
            </Empty>
          )}
        </div>
      </div>
    );
  const allMaterial = records(rows, "resource").filter((r) =>
    r.data.spaceIds.includes(space.id),
  );
  const material = allMaterial
    .filter(
      (r) =>
        (filter === "all" || r.data.type === filter) &&
        (!pinned || r.data.pinned) &&
        (tag === "all" || r.data.tags?.includes(tag)) &&
        `${r.data.title} ${find(rows, r.data.latestRevisionId)?.data.text || ""} ${(r.data.tags || []).join(" ")}`
          .toLowerCase()
          .includes(search.toLowerCase()),
    )
    .sort(
      (a, b) =>
        Number(!!b.data.pinned) - Number(!!a.data.pinned) ||
        b.updatedAt.localeCompare(a.updatedAt),
    );
  const channels = records(rows, "conversation").filter(
    (c) => c.data.spaceId === space.id && c.data.kind !== "thread",
  );
  const projects = records(rows, "project").filter(
    (p) => p.data.spaceId === space.id,
  );
  const members = (space.data.agentIds || [])
    .map((id: string) => find(rows, id))
    .filter(Boolean);
  const tags = [
    ...new Set<string>(allMaterial.flatMap((r) => r.data.tags || [])),
  ];
  return (
    <div className="page space-board-page">
      <button className="back-link" onClick={() => onNavigate("Spaces")}>
        <ArrowLeft size={14} /> All spaces
      </button>
      <header className="space-board-heading">
        <span className="space-marker" style={{ background: space.data.color }}>
          <Layers size={23} />
        </span>
        <div>
          <h1>{space.data.name}</h1>
          <p>{space.data.description}</p>
        </div>
        <button
          className="quiet"
          onClick={() => openModal({ type: "space", edit: space })}
        >
          <Settings size={15} /> Space settings
        </button>
      </header>
      <div className="space-audience">
        <Users size={14} />
        <span>
          {members.length
            ? `You, ${members.map((m: Row) => m.data.name).join(", ")}`
            : "Only you · invite agents in Space settings"}
        </span>
      </div>
      <div className="space-content-layout">
        <section className="material-board">
          <div className="board-toolbar">
            <h2>
              Shared material <span>{allMaterial.length}</span>
            </h2>
            <div>
              <button
                className="secondary"
                onClick={() => openModal({ type: "space-existing", space })}
              >
                <Link2 size={14} /> Add existing
              </button>
              <button
                className="primary"
                onClick={() =>
                  openModal({ type: "resource", spaceId: space.id })
                }
              >
                <Plus size={15} /> Post to space
              </button>
            </div>
          </div>
          <div className="material-filters">
            <select
              aria-label="Filter material by type"
              value={filter}
              onChange={(e) => setFilter(e.target.value)}
            >
              <option value="all">All material</option>
              <option value="note">Notes</option>
              <option value="paste">Pastes</option>
              <option value="file">Files</option>
              <option value="output">Agent outputs</option>
            </select>
            <button
              className={pinned ? "filter-chip active" : "filter-chip"}
              aria-pressed={pinned}
              onClick={() => setPinned(!pinned)}
            >
              <Pin size={12} /> Pinned
            </button>
            {tags.length > 0 && (
              <select
                aria-label="Filter material by tag"
                value={tag}
                onChange={(e) => setTag(e.target.value)}
              >
                <option value="all">All tags</option>
                {tags.map((t) => (
                  <option key={t}>{t}</option>
                ))}
              </select>
            )}
          </div>
          <ErrorText text={op.error} />
          <div className="material-grid">
            {material.map((r) => {
              const rev = find(rows, r.data.latestRevisionId);
              return (
                <article
                  className={`material-item material-${r.data.type}`}
                  key={r.id}
                >
                  <button className="material-open" onClick={() => setPanel(r)}>
                    <div className="material-item-top">
                      <span>
                        {r.data.type === "output"
                          ? "Agent output"
                          : r.data.type}
                      </span>
                      {r.data.pinned && <Pin size={12} />}
                    </div>
                    <h3>{r.data.title}</h3>
                    {r.data.type === "file" ? (
                      <div className="material-file">
                        <FileText size={29} />
                        <span>
                          {rev?.data.filename}
                          <small>
                            {rev?.data.size < 1024
                              ? `${rev?.data.size} bytes`
                              : `${Math.ceil((rev?.data.size || 0) / 1024)} KB`}
                          </small>
                        </span>
                      </div>
                    ) : (
                      <p className="material-text">
                        {rev?.data.text ||
                          "An empty note, ready for your thoughts."}
                      </p>
                    )}
                    <div className="material-tags">
                      {(r.data.tags || []).map((t: string) => (
                        <span key={t}>{t}</span>
                      ))}
                    </div>
                  </button>
                  <footer>
                    <small>
                      v{rev?.data.number} · {date(r.updatedAt)}
                    </small>
                    <div>
                      <button
                        aria-label={`${r.data.pinned ? "Unpin" : "Pin"} ${r.data.title}`}
                        title={r.data.pinned ? "Unpin" : "Pin to top"}
                        disabled={op.pending}
                        onClick={() =>
                          op.run(() =>
                            command("resource.organize", {
                              id: r.id,
                              expectedRevision: r.revision,
                              spaceIds: r.data.spaceIds,
                              projectIds: r.data.projectIds,
                              tags: r.data.tags || [],
                              pinned: !r.data.pinned,
                            }),
                          )
                        }
                      >
                        <Pin size={13} />
                      </button>
                      <button
                        aria-label={`Attach ${r.data.title}`}
                        title="Attach to a message"
                        onClick={() =>
                          openModal({
                            type: "resource-attach",
                            resource: r,
                            spaceId: space.id,
                          })
                        }
                      >
                        <Link2 size={13} />
                      </button>
                      <button
                        aria-label={`Organize ${r.data.title}`}
                        title="Tags and places"
                        onClick={() =>
                          openModal({ type: "resource-organize", resource: r })
                        }
                      >
                        <MoreHorizontal size={14} />
                      </button>
                    </div>
                  </footer>
                </article>
              );
            })}
          </div>
          {!material.length && (
            <Empty
              title={
                allMaterial.length
                  ? "No material matches these filters"
                  : "Post the first useful thing"
              }
            >
              {allMaterial.length
                ? "Try another type or tag."
                : "A note, a useful file, a code paste, or an idea for an agent. Everything here has a stable Resource link and revision history."}
            </Empty>
          )}
        </section>
        <aside className="space-context">
          <div className="context-heading">
            <h3>Conversations</h3>
            <button
              aria-label="New space channel"
              onClick={() =>
                openModal({
                  type: "conversation",
                  kind: "channel",
                  spaceId: space.id,
                })
              }
            >
              <Plus size={15} />
            </button>
          </div>
          {channels.map((c) => {
            const latest = records(rows, "message")
              .filter((m) => m.data.conversationId === c.id && !m.data.staged)
              .at(-1);
            return (
              <button
                className="space-context-link"
                key={c.id}
                onClick={() => onNavigate("Messages", c.id)}
              >
                <Hash size={14} />
                <span>
                  <strong>{c.data.title}</strong>
                  <small>{latest?.data.body || "Open conversation"}</small>
                </span>
              </button>
            );
          })}
          {!channels.length && (
            <p className="context-empty">
              Create a channel to discuss this space's work.
            </p>
          )}
          <div className="context-heading">
            <h3>Projects</h3>
            <button
              aria-label="New space project"
              onClick={() => openModal({ type: "project", spaceId: space.id })}
            >
              <Plus size={15} />
            </button>
          </div>
          {projects.map((p) => (
            <button
              className="space-context-link"
              key={p.id}
              onClick={() => onNavigate("Projects", p.id)}
            >
              <FolderOpen size={14} />
              <span>
                <strong>{p.data.name}</strong>
              </span>
              <ChevronRight size={12} />
            </button>
          ))}
          {!projects.length && (
            <p className="context-empty">
              Projects give this space's plans an outcome.
            </p>
          )}
          <p className="space-access-note">
            <ShieldCheck size={14} /> Posts show their audience before sharing.
            Linking existing material keeps its identity and revision history.
          </p>
        </aside>
      </div>
    </div>
  );
}

export function PlanningViews({
  view,
  tasks,
  rows,
  search,
  filter,
  setPanel,
  command,
  project,
  openModal,
}: Props) {
  const op = useAction(),
    visible = tasks.filter(
      (t: Row) =>
        (filter === "all" || filter === t.data.status) &&
        `${t.data.title} ${t.data.description}`
          .toLowerCase()
          .includes(search.toLowerCase()),
    );
  const reasons = (t: Row) =>
    (
      t.data.readiness?.reasons ||
      [t.data.blocker, t.data.waitingReason].filter(Boolean)
    ).filter(
      (reason: string) =>
        !/^Task is (accepted|running|review|cancelled)$/.test(reason),
    );
  if (view === "Board")
    return (
      <>
        <ErrorText text={op.error} />
        <div className="task-board" aria-label="Project task board">
          {["draft", "ready", "running", "review", "accepted", "cancelled"]
            .filter(
              (status) =>
                status !== "cancelled" ||
                visible.some((t: Row) => t.data.status === "cancelled"),
            )
            .map((status) => (
              <section className="task-board-column" key={status}>
                <header>
                  <State status={status} />
                  <span>
                    {
                      visible.filter((t: Row) => t.data.status === status)
                        .length
                    }
                  </span>
                </header>
                {visible
                  .filter((t: Row) => t.data.status === status)
                  .map((t: Row) => (
                    <article className="task-board-card" key={t.id}>
                      <button
                        className="task-board-open"
                        onClick={() => setPanel(t)}
                      >
                        <small>{t.data.kind}</small>
                        <h3>{t.data.title}</h3>
                        {t.data.assigneeId && (
                          <span className="task-assignee">
                            <Users size={11} />
                            {find(rows, t.data.assigneeId)?.data.name}
                          </span>
                        )}
                        {reasons(t).length > 0 && <p>{reasons(t)[0]}</p>}
                        <div className="board-task-meta">
                          {t.data.criteria.length > 0 && (
                            <span>{t.data.criteria.length} criteria</span>
                          )}
                          {t.data.dependencies.length > 0 && (
                            <span>
                              <Link2 size={11} />
                              {t.data.dependencies.length}
                            </span>
                          )}
                        </div>
                      </button>
                      {["draft", "ready"].includes(status) && (
                        <select
                          aria-label={`Change status of ${t.data.title}`}
                          value={status}
                          disabled={op.pending}
                          onChange={(e) =>
                            op.run(() =>
                              command("task.update", {
                                id: t.id,
                                expectedRevision: t.revision,
                                status: e.target.value,
                              }),
                            )
                          }
                        >
                          <option value="draft">Draft</option>
                          <option value="ready">Ready</option>
                          <option value="cancelled">Cancelled</option>
                        </select>
                      )}
                    </article>
                  ))}
                {status === "draft" && (
                  <button
                    className="add-board-task"
                    onClick={() =>
                      openModal({ type: "task", projectId: project.id })
                    }
                  >
                    <Plus size={13} /> Add task
                  </button>
                )}
              </section>
            ))}
        </div>
      </>
    );
  return (
    <div className="dependency-view">
      <p className="view-explanation">
        Prerequisites must be accepted before dependent work starts. Code also
        waits for verified integration.
      </p>
      <div className="dependency-heading">
        <span>Prerequisites</span>
        <span>Task</span>
        <span>Unblocks</span>
      </div>
      {visible.map((t: Row) => {
        const deps = t.data.dependencies
            .map((id: string) => find(rows, id))
            .filter(Boolean),
          downstream = records(rows, "task").filter((r) =>
            r.data.dependencies.includes(t.id),
          );
        return (
          <div className="dependency-row" key={t.id}>
            <div className="dependency-links">
              {deps.length ? (
                deps.map((d: Row) => (
                  <button key={d.id} onClick={() => setPanel(d)}>
                    <span
                      className={`dependency-dot ${d.data.status === "accepted" && (d.data.execution !== "code" || d.data.integratedCommit) ? "satisfied" : ""}`}
                    />
                    {d.data.title}
                    <small>
                      {d.data.status === "accepted" &&
                      d.data.execution === "code" &&
                      !d.data.integratedCommit
                        ? "Integration needed"
                        : d.data.status}
                    </small>
                  </button>
                ))
              ) : (
                <span className="dependency-none">No prerequisites</span>
              )}
            </div>
            <button className="dependency-main" onClick={() => setPanel(t)}>
              <ArrowRight size={14} />
              <span>
                <strong>{t.data.title}</strong>
                <State status={t.data.status} />
                {reasons(t)[0] && <small>{reasons(t)[0]}</small>}
              </span>
              <ArrowRight size={14} />
            </button>
            <div className="dependency-links">
              {downstream.length ? (
                downstream.map((d) => (
                  <button key={d.id} onClick={() => setPanel(d)}>
                    {d.data.title}
                    <small>{d.data.status}</small>
                  </button>
                ))
              ) : (
                <span className="dependency-none">No dependents yet</span>
              )}
            </div>
          </div>
        );
      })}
      {!visible.length && (
        <Empty title="No tasks to map">
          Create a task, then add its prerequisites in task details.
        </Empty>
      )}
    </div>
  );
}

export function ResourceAttachmentForm({
  modal,
  rows,
  command,
  close,
  onNavigate,
}: Props) {
  const resource = find(rows, modal.resource.id) || modal.resource,
    [conversationId, setConversation] = useState(""),
    [body, setBody] = useState(""),
    [revisionId] = useState(modal.revisionId || resource.data.latestRevisionId),
    op = useAction();
  const conversation = find(rows, conversationId),
    members = conversation?.data.agentIds || [];
  const needsGrant =
    !!conversation &&
    (!resource.data.conversationIds.includes(conversationId) ||
      members.some((id: string) => !resource.data.agentIds.includes(id)));
  return (
    <form
      onSubmit={(e) => {
        e.preventDefault();
        op.run(async () => {
          if (needsGrant)
            await command("resource.grant", {
              id: resource.id,
              expectedRevision: resource.revision,
              conversationIds: [
                ...new Set([...resource.data.conversationIds, conversationId]),
              ],
              agentIds: [...new Set([...resource.data.agentIds, ...members])],
              spaceIds: resource.data.spaceIds,
              projectIds: resource.data.projectIds,
            });
          await command("message.send", {
            conversationId,
            body: body.trim() || `Shared material: ${resource.data.title}`,
            attachments: [{ resourceId: resource.id, revisionId }],
          });
          close();
          onNavigate("Messages", conversationId);
        });
      }}
    >
      <div className="attach-summary">
        <FileText size={20} />
        <span>
          <strong>{resource.data.title}</strong>
          <small>
            Revision {find(rows, revisionId)?.data.number} is pinned to the
            message.
          </small>
        </span>
      </div>
      <label className="field">
        <span>Conversation</span>
        <select
          required
          value={conversationId}
          onChange={(e) => setConversation(e.target.value)}
        >
          <option value="">Choose a conversation</option>
          {records(rows, "conversation")
            .sort(
              (a, b) =>
                Number(b.data.spaceId === modal.spaceId) -
                Number(a.data.spaceId === modal.spaceId),
            )
            .map((c) => (
              <option key={c.id} value={c.id}>
                {c.data.kind === "channel" ? "# " : ""}
                {c.data.title}
              </option>
            ))}
        </select>
      </label>
      <label className="field">
        <span>Message</span>
        <textarea
          value={body}
          onChange={(e) => setBody(e.target.value)}
          rows={4}
          placeholder="What should the agents know about this?"
        />
      </label>
      {conversation && (
        <p className="audience-notice">
          <Users size={14} />
          <span>
            {needsGrant
              ? "This also shares the Resource with"
              : "Accessible to"}{" "}
            {members.map((id: string) => find(rows, id)?.data.name).join(", ")}{" "}
            in {conversation.data.title}.
          </span>
        </p>
      )}
      <ErrorText text={op.error} />
      <div className="form-actions">
        <button className="primary" disabled={!conversationId || op.pending}>
          {op.pending && <Loader2 size={14} className="spin" />}
          {needsGrant ? "Share & send" : "Send attachment"}
        </button>
      </div>
    </form>
  );
}

export function ResourceOrganizationForm({
  modal,
  rows,
  command,
  close,
}: Props) {
  const r = modal.resource,
    [spaceIds, setSpaces] = useState<string[]>(r.data.spaceIds),
    [projectIds, setProjects] = useState<string[]>(r.data.projectIds),
    [tags, setTags] = useState((r.data.tags || []).join(", ")),
    [pinned, setPinned] = useState(!!r.data.pinned),
    op = useAction();
  const checks = (
    kind: string,
    values: string[],
    set: (values: string[]) => void,
  ) => (
    <fieldset className="check-list">
      <legend>{kind === "space" ? "Spaces" : "Projects"}</legend>
      {records(rows, kind).map((item) => (
        <label className="check-option" key={item.id}>
          <input
            type="checkbox"
            checked={values.includes(item.id)}
            onChange={(e) =>
              set(
                e.target.checked
                  ? [...values, item.id]
                  : values.filter((id) => id !== item.id),
              )
            }
          />
          <span>{item.data.name}</span>
        </label>
      ))}
    </fieldset>
  );
  return (
    <form
      onSubmit={(e) => {
        e.preventDefault();
        op.run(async () => {
          await command("resource.organize", {
            id: r.id,
            expectedRevision: r.revision,
            spaceIds,
            projectIds,
            pinned,
            tags: tags
              .split(",")
              .map((v: string) => v.trim())
              .filter(Boolean),
          });
          close();
        });
      }}
    >
      <p className="muted small">
        Organize “{r.data.title}” without making a copy. These links do not
        grant new access.
      </p>
      <label className="field">
        <span>Tags</span>
        <input
          value={tags}
          onChange={(e) => setTags(e.target.value)}
          placeholder="design, research, reference"
        />
        <small>Separate tags with commas.</small>
      </label>
      <label className="check-option">
        <input
          type="checkbox"
          checked={pinned}
          onChange={(e) => setPinned(e.target.checked)}
        />
        <span>Pin to the top of its spaces</span>
      </label>
      {checks("space", spaceIds, setSpaces)}
      {checks("project", projectIds, setProjects)}
      <ErrorText text={op.error} />
      <div className="form-actions">
        <button className="primary" disabled={op.pending}>
          Save organization
        </button>
      </div>
    </form>
  );
}

export function SpaceExistingForm({ modal, rows, command, close }: Props) {
  const space = find(rows, modal.space.id) || modal.space,
    [ids, setIds] = useState<string[]>([]),
    op = useAction();
  const candidates = records(rows, "resource").filter(
      (r) => !r.data.spaceIds.includes(space.id) && !r.data.stagedFor,
    ),
    agentIds = space.data.agentIds || [];
  return (
    <form
      onSubmit={(e) => {
        e.preventDefault();
        op.run(async () => {
          for (const id of ids) {
            const r = find(rows, id)!;
            const organized = await command("resource.organize", {
              id,
              expectedRevision: r.revision,
              spaceIds: [...r.data.spaceIds, space.id],
              projectIds: r.data.projectIds,
              pinned: r.data.pinned || false,
              tags: r.data.tags || [],
            });
            if (agentIds.length)
              await command("resource.grant", {
                id,
                expectedRevision: organized.revision,
                conversationIds: r.data.conversationIds,
                agentIds: [...new Set([...r.data.agentIds, ...agentIds])],
                spaceIds: organized.data.spaceIds,
                projectIds: r.data.projectIds,
              });
          }
          close();
        });
      }}
    >
      <p className="muted small">
        Choose existing Resources to post in {space.data.name}. Their revisions
        and existing attachments stay intact.
      </p>
      <fieldset className="check-list">
        {candidates.map((r) => (
          <label className="check-option" key={r.id}>
            <input
              type="checkbox"
              checked={ids.includes(r.id)}
              onChange={(e) =>
                setIds(
                  e.target.checked
                    ? [...ids, r.id]
                    : ids.filter((id) => id !== r.id),
                )
              }
            />
            <span>
              {r.data.title}
              <small>
                {r.data.type} · revision{" "}
                {find(rows, r.data.latestRevisionId)?.data.number}
              </small>
            </span>
          </label>
        ))}
        {!candidates.length && (
          <p className="muted small">
            Every available Resource is already in this space.
          </p>
        )}
      </fieldset>
      <p className="audience-notice">
        <Users size={14} />
        <span>
          {agentIds.length
            ? `This shares the selected material with ${agentIds.map((id: string) => find(rows, id)?.data.name).join(", ")}.`
            : "This space is currently private to you."}
        </span>
      </p>
      <ErrorText text={op.error} />
      <div className="form-actions">
        <button className="primary" disabled={!ids.length || op.pending}>
          Post {ids.length || ""} {ids.length === 1 ? "Resource" : "Resources"}
        </button>
      </div>
    </form>
  );
}

export function AgentConnection({ row, command, setNotice }: Props) {
  const [mode, setMode] = useState(row.data.deliveryMode || "managed"),
    [minutes, setMinutes] = useState(row.data.checkInMinutes || 5),
    [url, setUrl] = useState(row.data.webhookUrl || ""),
    [secret, setSecret] = useState(""),
    [credentials, setCredentials] = useState<any[]>([]),
    [label, setLabel] = useState(`${row.data.name} connection`),
    [days, setDays] = useState(30),
    [issued, setIssued] = useState<any>(null),
    [showIssue, setShowIssue] = useState(false),
    [showToken, setShowToken] = useState(false),
    [configured, setConfigured] = useState(row),
    [base, setBase] = useState(row.revision),
    op = useAction();
  const latest = row.revision >= configured.revision ? row : configured;
  const refresh = async () => {
    const result = await command("credential.list", { agentId: row.id });
    setCredentials(Array.isArray(result) ? result : result.credentials || []);
  };
  useEffect(() => {
    void op.run(refresh);
  }, [row.id]);
  const rawToken = issued?.token || issued?.credential?.token;
  return (
    <div className="agent-connection">
      <form
        onSubmit={(e) => {
          e.preventDefault();
          op.run(async () => {
            if (mode === "webhook") {
              const target = new URL(url);
              if (
                target.protocol !== "https:" &&
                !(
                  target.protocol === "http:" &&
                  ["localhost", "127.0.0.1", "[::1]"].includes(target.hostname)
                )
              )
                throw new Error("Remote webhook URLs must use HTTPS.");
            }
            const result = await command("agent.configure", {
              id: row.id,
              expectedRevision: base,
              deliveryMode: mode,
              checkInMinutes: minutes,
              ...(mode === "webhook"
                ? {
                    webhookUrl: url,
                    ...(secret ? { webhookSecret: secret } : {}),
                  }
                : {}),
            });
            setConfigured(result);
            setBase(result.revision);
            setSecret("");
            setNotice("Agent connection updated.");
          });
        }}
      >
        <label className="field">
          <span>How this agent receives work</span>
          <select value={mode} onChange={(e) => setMode(e.target.value)}>
            <option value="managed">Local Codex · responds on demand</option>
            <option value="poll">Connected agent · checks its inbox</option>
            <option value="webhook">
              Connected agent · webhook notification
            </option>
          </select>
          <small>
            {mode === "managed"
              ? "The workspace starts an isolated Codex session when needed."
              : mode === "poll"
                ? "Give the agent a token, then configure its client to check its inbox. The workspace does not start those scheduled check-ins."
                : "The workspace notifies your agent when work arrives. It uses its token to read and claim work."}
          </small>
        </label>
        {mode !== "managed" && (
          <label className="field">
            <span>Expected check-in interval</span>
            <select
              value={minutes}
              onChange={(e) => setMinutes(Number(e.target.value))}
            >
              {[...new Set([1, 5, 15, 30, 60, 1440, 10080, minutes])]
                .sort((a, b) => a - b)
                .map((m) => (
                  <option key={m} value={m}>
                    {m === 1440
                      ? "Every day"
                      : m === 10080
                        ? "Every week"
                        : m === 60
                          ? "Every hour"
                          : m === 1
                            ? "Every minute"
                            : `Every ${m} minutes`}
                  </option>
                ))}
            </select>
            <small>
              This describes the agent's schedule. Configure its client to check
              in at that interval.
            </small>
          </label>
        )}
        {mode === "webhook" && (
          <>
            <label className="field">
              <span>Webhook URL</span>
              <input
                type="url"
                required
                value={url}
                onChange={(e) => setUrl(e.target.value)}
                placeholder="https://your-agent.example/workspace"
              />
            </label>
            <label className="field">
              <span>Webhook secret</span>
              <input
                type="password"
                autoComplete="new-password"
                minLength={16}
                maxLength={512}
                required={!latest.data.webhookConfigured}
                value={secret}
                onChange={(e) => setSecret(e.target.value)}
                placeholder={
                  latest.data.webhookConfigured
                    ? "Configured · leave blank to keep"
                    : "Enter a secret for webhook verification"
                }
              />
              <small>
                Use 16–512 characters. The saved secret is never shown again.
              </small>
            </label>
          </>
        )}
        <div className="connection-meta">
          <Clock size={13} />
          <span>Last seen {date(row.data.lastSeenAt)}</span>
        </div>
        {latest.data.lastWakeupStatus && (
          <div
            className={
              latest.data.lastWakeupStatus === "failed"
                ? "notice-box warning"
                : "notice-box"
            }
          >
            <strong>
              Last webhook notification:{" "}
              {String(latest.data.lastWakeupStatus).replaceAll("_", " ")}
            </strong>
            {latest.data.lastWakeupError && (
              <p>{latest.data.lastWakeupError}</p>
            )}
            {latest.data.lastWakeupStatus === "delivered" && (
              <p>
                The endpoint accepted the notification. Last seen records when
                the agent actually checked in.
              </p>
            )}
          </div>
        )}
        <div className="form-actions">
          <button className="primary" disabled={op.pending}>
            Save connection
          </button>
        </div>
      </form>
      <section className="agent-token-section">
        <div className="context-heading">
          <h3>Access tokens</h3>
          <button
            className="secondary"
            onClick={() => {
              setShowIssue(!showIssue);
              setIssued(null);
              setShowToken(false);
            }}
          >
            <Plus size={13} /> New token
          </button>
        </div>
        <p className="muted small">
          Each token acts as {row.data.name}. Access is limited to this agent's
          conversations, spaces, projects and shared material.
        </p>
        {showIssue && !issued && (
          <form
            className="token-issue-form"
            onSubmit={(e) => {
              e.preventDefault();
              op.run(async () => {
                const result = await command("credential.issue", {
                  agentId: row.id,
                  label,
                  expiresInDays: days,
                });
                setIssued(result);
                await refresh();
              });
            }}
          >
            <label className="field">
              <span>Token label</span>
              <input
                required
                value={label}
                onChange={(e) => setLabel(e.target.value)}
                maxLength={120}
              />
            </label>
            <label className="field">
              <span>Expires after</span>
              <select
                value={days}
                onChange={(e) => setDays(Number(e.target.value))}
              >
                <option value={7}>7 days</option>
                <option value={30}>30 days</option>
                <option value={90}>90 days</option>
                <option value={0}>Until revoked</option>
              </select>
            </label>
            <button className="primary" disabled={op.pending}>
              <KeyRound size={14} /> Issue token
            </button>
          </form>
        )}
        {issued && (
          <div className="issued-token">
            {rawToken ? (
              <>
                <strong>Copy this token now</strong>
                <p>
                  It is shown only once. Keep it in your agent's secret
                  configuration.
                </p>
                <div className="token-copy">
                  <input
                    aria-label="New agent token"
                    readOnly
                    type={showToken ? "text" : "password"}
                    value={rawToken}
                  />
                  <button
                    className="secondary"
                    onClick={() =>
                      op.run(async () => {
                        await navigator.clipboard.writeText(rawToken);
                        setNotice("Token copied.");
                      })
                    }
                  >
                    <Copy size={14} /> Copy
                  </button>
                </div>
                <button
                  className="quiet"
                  onClick={() => setShowToken(!showToken)}
                >
                  {showToken ? "Hide token" : "Show token"}
                </button>
                <button
                  className="quiet"
                  onClick={() => {
                    setIssued(null);
                    setShowIssue(false);
                    setShowToken(false);
                  }}
                >
                  Done
                </button>
              </>
            ) : (
              <p>
                The token was issued earlier and cannot be displayed again.
                Issue a new token if you did not keep it.
              </p>
            )}
          </div>
        )}
        <div className="credential-list">
          {credentials.map((c) => (
            <div className="credential-row" key={c.id}>
              <KeyRound size={15} />
              <span>
                <strong>{c.label || "Agent token"}</strong>
                <small>
                  {c.revokedAt
                    ? "Revoked"
                    : c.expiresAt?.startsWith("9999-")
                      ? "Until revoked"
                      : c.expiresAt
                      ? `Expires ${date(c.expiresAt)}`
                      : "No expiration recorded"}
                  {c.lastSeenAt ? ` · used ${date(c.lastSeenAt)}` : ""}
                </small>
              </span>
              {!c.revokedAt && (
                <button
                  className="quiet"
                  disabled={op.pending}
                  aria-label={`Revoke ${c.label || "token"}`}
                  onClick={() =>
                    op.run(async () => {
                      await command("credential.revoke", { id: c.id });
                      await refresh();
                      setNotice("Token revoked.");
                    })
                  }
                >
                  Revoke
                </button>
              )}
            </div>
          ))}
          {!credentials.length && !op.pending && (
            <p className="muted small">No tokens issued for this agent.</p>
          )}
        </div>
        <details className="connection-instructions">
          <summary>Connection details</summary>
          <p className="muted small">
            Use a bearer token with the agent endpoint or connect an MCP client.
            These endpoints apply the same permissions.
          </p>
          <label className="field">
            <span>MCP endpoint</span>
            <input readOnly value={`${window.location.origin}/mcp`} />
          </label>
          <label className="field">
            <span>Agent command endpoint</span>
            <input
              readOnly
              value={`${window.location.origin}/api/agent/commands`}
            />
          </label>
          <p className="muted small">
            A client checks <code>inbox.list</code>, claims a request, then
            sends its response. If connecting from another host, use the
            deployment's reachable address.
          </p>
        </details>
      </section>
      <ErrorText text={op.error} />
    </div>
  );
}
