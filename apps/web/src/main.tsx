import React, { useState, useEffect, useRef, useCallback } from "react";
import { createRoot } from "react-dom/client";
import {
  MessageSquare,
  Inbox,
  FolderOpen,
  Layers,
  Users,
  Plus,
  ArrowUp,
  Search,
  ChevronDown,
  ChevronRight,
  Check,
  Square,
  MoreHorizontal,
  Paperclip,
  FileText,
  ArrowLeft,
  Play,
  Pause,
  Link2,
  X,
  ArrowRight,
  RefreshCw,
  Settings,
  LogOut,
  CornerUpLeft,
  Forward,
  BookOpen,
  AlertCircle,
  CheckCircle2,
  Clock,
  GitBranch,
} from "lucide-react";
import type {
  Snapshot,
  RecordRow,
  Attachment,
  Task,
  Conversation,
  Message,
} from "../../../packages/shared/src/index";
import "./style.css";
import {
  ProjectView,
  TaskRow,
  DetailPanel,
  ModalContent,
  RichText,
} from "./components";
type Row = RecordRow<any>;
const navigation = [
  ["Messages", MessageSquare],
  ["Inbox", Inbox],
  ["Resources", FileText],
  ["Spaces", Layers],
  ["Projects", FolderOpen],
  ["Agents", Users],
] as const;
const empty: Snapshot = {
  records: [],
  cursor: 0,
  dispatchEnabled: true,
  workerOnline: false,
  runtime: {},
  github: {},
};
function App() {
  const [auth, setAuth] = useState<{
      authenticated: boolean;
      csrf?: string;
    } | null>(null),
    [token, setToken] = useState(""),
    [state, setState] = useState<Snapshot>(empty),
    [section, setSection] = useState(
      localStorage.getItem("section") || "Messages",
    ),
    [selected, setSelected] = useState(localStorage.getItem("selected") || ""),
    [error, setError] = useState(""),
    [notice, setNotice] = useState(""),
    [busy, setBusy] = useState(false),
    [modal, setModal] = useState<any>(null),
    [panel, setPanel] = useState<(Row & { viewRevisionId?: string }) | null>(
      null,
    ),
    [search, setSearch] = useState(""),
    [connected, setConnected] = useState(false);
  const fetcher = async (url: string, opts?: RequestInit) => {
    const res = await fetch(url, opts);
    const body = await res.json();
    if (!res.ok) {
      if (res.status === 401) setAuth({ authenticated: false });
      throw new Error(body.error || "Request failed");
    }
    return body;
  };
  const refresh = useCallback(async () => {
    try {
      const next = await fetcher("/api/snapshot");
      setState(next);
    } catch (e: any) {
      setError(e.message);
    }
  }, []);
  useEffect(() => {
    fetcher("/api/session")
      .then(setAuth)
      .catch((e) => setError(e.message));
  }, []);
  useEffect(() => {
    if (!auth?.authenticated) return;
    refresh();
    const heartbeat = setInterval(refresh, 15000);
    return () => clearInterval(heartbeat);
  }, [auth?.authenticated, refresh]);
  useEffect(() => {
    if (!auth?.authenticated) return;
    const es = new EventSource(`/api/events?after=${state.cursor}`);
    let timer: any;
    const update = () => {
      clearTimeout(timer);
      timer = setTimeout(refresh, 100);
    };
    es.addEventListener("change", update);
    es.addEventListener("reconcile", update);
    es.onopen = () => {
      setConnected(true);
      refresh();
    };
    es.onerror = () => setConnected(false);
    return () => {
      es.close();
      clearTimeout(timer);
    };
  }, [auth?.authenticated]);
  useEffect(() => {
    localStorage.setItem("section", section);
    localStorage.setItem("selected", selected);
  }, [section, selected]);
  useEffect(() => {
    if (notice) {
      const timer = setTimeout(() => setNotice(""), 5000);
      return () => clearTimeout(timer);
    }
  }, [notice]);
  const all = (kind: string) => state.records.filter((r) => r.kind === kind),
    get = (id: string) => state.records.find((r) => r.id === id),
    name = (id: string) =>
      id === "owner"
        ? "You"
        : id === "system"
          ? "Workspace"
          : get(id)?.data.name || "Agent";
  const command = async (
    type: string,
    input: any,
    id = crypto.randomUUID(),
  ) => {
    setBusy(true);
    setError("");
    try {
      const result = await fetcher("/api/commands", {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          "X-CSRF-Token": auth?.csrf || "",
        },
        body: JSON.stringify({ id, type, input }),
      });
      await refresh();
      return result.value;
    } catch (e: any) {
      setError(e.message);
      throw e;
    } finally {
      setBusy(false);
    }
  };
  const go = (next: string, id = "") => {
    setSection(next);
    setSelected(id);
    setPanel(null);
    setSearch("");
  };
  const showResource = (id: string, revisionId?: string) => {
    const row = get(id);
    if (row) setPanel({ ...row, viewRevisionId: revisionId });
  };
  const view =
    selected && get(selected)?.kind === "conversation"
      ? get(selected)
      : all("conversation")[0];
  const activeRequests = all("request").filter((r) =>
    ["pending", "claimed", "running", "needs_reconciliation"].includes(
      r.data.status,
    ),
  );
  const attentionCount =
    all("proposal").filter((p) => p.data.status === "proposed").length +
    all("task").filter((t) => t.data.status === "review").length +
    all("run").filter((r) => r.data.status === "needs_reconciliation").length;
  const actions = async (e: React.FormEvent, fn: () => Promise<any>) => {
    e.preventDefault();
    try {
      await fn();
    } catch {}
  };
  const modalDone = async (type: string, input: any) => {
    const row = await command(type, input);
    setModal(null);
    return row;
  };
  const filtered = (rows: Row[]) =>
    rows.filter((r) =>
      JSON.stringify([r.data.name, r.data.title, r.data.description])
        .toLowerCase()
        .includes(search.toLowerCase()),
    );
  if (!auth) return <div className="loading">Opening your workspace…</div>;
  if (!auth.authenticated)
    return (
      <main className="login">
        <div className="login-box">
          <div className="brand-symbol">w</div>
          <p className="eyebrow">YOUR PRIVATE WORKSPACE</p>
          <h1>
            A place to think.
            <br />A place to get things done.
          </h1>
          <p>
            Talk with your agents, keep useful material, and turn ideas into
            work you can follow.
          </p>
          <form
            onSubmit={(e) =>
              actions(e, async () => {
                setAuth(
                  await fetcher("/api/login", {
                    method: "POST",
                    headers: { "Content-Type": "application/json" },
                    body: JSON.stringify({ token }),
                  }),
                );
                setToken("");
              })
            }
          >
            <label>
              Workspace access token
              <input
                type="password"
                autoComplete="current-password"
                value={token}
                onChange={(e) => setToken(e.target.value)}
                placeholder="Paste your local access token"
                required
              />
            </label>
            <button className="primary">
              Open workspace <ArrowRight size={16} />
            </button>
          </form>
          {error && <p className="error">{error}</p>}
          <small>
            Your token is saved on this Mac in <code>.local/owner-token</code>.
            This app is available only on this computer.
          </small>
        </div>
      </main>
    );
  return (
    <div className="app-shell">
      <aside className="sidebar">
        <button className="brand" onClick={() => go("Messages")}>
          <span className="brand-symbol">w</span>
          <span>
            Workspace<small>YOUR AGENTS, TOGETHER</small>
          </span>
          <ChevronDown size={14} />
        </button>
        <nav>
          {navigation.map(([label, Icon]) => (
            <button
              key={label}
              className={section === label ? "nav active" : "nav"}
              onClick={() => go(label)}
            >
              <Icon size={18} />
              <span>{label}</span>
              {label === "Inbox" && attentionCount > 0 && (
                <b>{attentionCount}</b>
              )}
            </button>
          ))}
        </nav>
        <div className="sidebar-section">
          <span>YOUR PROJECTS</span>
          <button
            aria-label="New project"
            onClick={() => setModal({ type: "project" })}
          >
            <Plus size={15} />
          </button>
        </div>
        {all("project")
          .slice(0, 8)
          .map((r) => (
            <button
              className="project-nav"
              key={r.id}
              onClick={() => go("Projects", r.id)}
            >
              <span className="dot" />
              {r.data.name}
            </button>
          ))}
        {!all("project").length && (
          <p className="sidebar-hint">Make room for your next idea.</p>
        )}
        <div className="sidebar-bottom">
          <span className={`connection ${connected ? "online" : ""}`} />
          <small>
            {!connected
              ? "Reconnecting"
              : state.workerOnline
                ? "Local workspace"
                : "Worker offline"}
          </small>
          <button
            title="Workspace settings"
            onClick={() => setModal({ type: "settings" })}
          >
            <Settings size={16} />
          </button>
        </div>
        <button
          className="owner"
          onClick={() => setModal({ type: "settings" })}
        >
          <span className="avatar owner-avatar">Y</span>
          <span>
            Your workspace<small>Private · on this Mac</small>
          </span>
        </button>
      </aside>
      <main className="main">
        <header className="topbar">
          <div>
            <span className="breadcrumb">Workspace</span>
            <ChevronRight size={14} />
            <strong>{section}</strong>
          </div>
          <div className="topbar-actions">
            <label className="search">
              <Search size={15} />
              <input
                aria-label="Search this view"
                placeholder="Find in this view"
                value={search}
                onChange={(e) => setSearch(e.target.value)}
              />
            </label>
            <button
              className="quiet"
              onClick={() => setModal({ type: "task" })}
            >
              <Plus size={16} /> Quick task
            </button>
            <button
              className="mobile-settings"
              aria-label="Workspace settings"
              onClick={() => setModal({ type: "settings" })}
            >
              <Settings size={16} />
            </button>
          </div>
        </header>
        {error && (
          <div role="alert" className="banner error">
            <AlertCircle size={16} />
            {error}
            <button aria-label="Dismiss error" onClick={() => setError("")}>
              <X size={16} />
            </button>
          </div>
        )}
        {notice && (
          <div role="status" className="toast">
            {notice}
          </div>
        )}
        {!state.dispatchEnabled && (
          <div className="banner">
            Agent dispatch is paused. Your messages and work are saved.
            <button onClick={() => setModal({ type: "settings" })}>
              Settings
            </button>
          </div>
        )}
        {section === "Messages" && (
          <div className="messages-layout">
            <aside className="conversation-list">
              <div className="list-heading">
                <h2>Conversations</h2>
                <button
                  aria-label="New conversation"
                  onClick={() => setModal({ type: "conversation" })}
                >
                  <Plus size={19} />
                </button>
              </div>
              {filtered(all("conversation")).map((c) => {
                const members = c.data.agentIds.map((id: string) => name(id));
                const latest = all("message")
                  .filter((m) => m.data.conversationId === c.id)
                  .at(-1);
                return (
                  <button
                    key={c.id}
                    className={`conversation-item ${view?.id === c.id ? "selected" : ""}`}
                    onClick={() => {
                      setSelected(c.id);
                      setPanel(null);
                    }}
                  >
                    <span
                      className={`avatar ${c.data.kind === "group" ? "group-avatar" : ""}`}
                    >
                      {c.data.kind === "group" ? (
                        <Users size={18} />
                      ) : (
                        members[0]?.[0]
                      )}
                    </span>
                    <span>
                      <strong>{c.data.title}</strong>
                      <small>{latest?.data.body || members.join(" & ")}</small>
                    </span>
                    {activeRequests.some(
                      (r) => r.data.conversationId === c.id,
                    ) && <span className="activity-dot" />}
                  </button>
                );
              })}
              <p className="list-footer">
                A conversation keeps its own context.
                <br />
                Shared Resources travel by choice.
              </p>
            </aside>
            {view ? (
              <Chat
                key={view.id}
                conversation={view}
                rows={state.records}
                command={command}
                name={name}
                get={get}
                showResource={showResource}
                busy={busy}
                openModal={setModal}
                setPanel={setPanel}
              />
            ) : (
              <Empty
                icon={MessageSquare}
                title="Start a conversation"
                detail="Choose an agent or bring a few together."
                action="New conversation"
                onClick={() => setModal({ type: "conversation" })}
              />
            )}
          </div>
        )}
        {section === "Inbox" && (
          <div className="page">
            <PageTitle
              eyebrow="KEEP THINGS MOVING"
              title="Your attention, where it matters"
              description="Questions, work ready to review, and anything that needs a decision."
            />
            <div className="inbox-list">
              {all("run")
                .filter((r) => r.data.status === "needs_reconciliation")
                .map((r) => (
                  <button
                    className="inbox-row"
                    key={r.id}
                    onClick={() => setPanel(r)}
                  >
                    <AlertCircle />
                    <span>
                      <strong>An interrupted run needs a check</strong>
                      <small>{r.data.error}</small>
                    </span>
                    <ArrowRight />
                  </button>
                ))}
              {all("proposal")
                .filter((p) => p.data.status === "proposed")
                .map((p) => (
                  <button
                    className="inbox-row"
                    key={p.id}
                    onClick={() => setPanel(p)}
                  >
                    <Layers />
                    <span>
                      <strong>{p.data.title}</strong>
                      <small>Plan proposal · review before adding tasks</small>
                    </span>
                    <ArrowRight />
                  </button>
                ))}
              {all("task")
                .filter((t) => t.data.status === "review")
                .map((t) => (
                  <button
                    className="inbox-row"
                    key={t.id}
                    onClick={() => setPanel(t)}
                  >
                    <CheckCircle2 />
                    <span>
                      <strong>{t.data.title}</strong>
                      <small>Ready for your review</small>
                    </span>
                    <ArrowRight />
                  </button>
                ))}
              {!all("run").some(
                (r) => r.data.status === "needs_reconciliation",
              ) &&
                !all("proposal").some((p) => p.data.status === "proposed") &&
                !all("task").some((t) => t.data.status === "review") && (
                  <Empty
                    icon={CheckCircle2}
                    title="You're all caught up"
                    detail="Plans and completed work will arrive here for your review."
                  />
                )}
            </div>
            <h3 className="subheading">In progress</h3>
            {activeRequests.map((r) => (
              <button
                className="inbox-row"
                key={r.id}
                onClick={() => go("Messages", r.data.conversationId)}
              >
                <Clock />
                <span>
                  <strong>
                    {name(r.data.agentId)} ·{" "}
                    {get(r.data.conversationId)?.data.title}
                  </strong>
                  <small>
                    {r.data.status.replaceAll("_", " ")} · {r.data.reason}
                  </small>
                </span>
              </button>
            ))}
          </div>
        )}
        {section === "Resources" && (
          <div className="page">
            <PageTitle
              eyebrow="THE SHARED SHELF"
              title="Keep what’s useful"
              description="Notes, files and ideas you and your agents can return to."
              action="New Resource"
              onClick={() => setModal({ type: "resource" })}
            />
            <div className="resource-grid">
              {filtered(all("resource")).map((r) => (
                <button
                  className="resource-card"
                  key={r.id}
                  onClick={() => setPanel(r)}
                >
                  <span className="resource-icon">
                    <FileText size={21} />
                  </span>
                  <small>
                    {r.data.type.toUpperCase()} ·{" "}
                    {r.data.stagedFor
                      ? "PRIVATE DRAFT"
                      : r.data.conversationIds.length
                        ? "SHARED"
                        : r.data.projectIds.length
                          ? "PROJECT"
                          : "ONLY YOU"}
                  </small>
                  <h3>{r.data.title}</h3>
                  <p>
                    {get(r.data.latestRevisionId)?.data.text ||
                      get(r.data.latestRevisionId)?.data.filename ||
                      "File Resource"}
                  </p>
                  <footer>
                    {new Date(r.updatedAt).toLocaleDateString(undefined, {
                      month: "short",
                      day: "numeric",
                    })}
                    <span>
                      Revision {get(r.data.latestRevisionId)?.data.number}
                    </span>
                  </footer>
                </button>
              ))}
            </div>
            {!all("resource").length && (
              <Empty
                icon={BookOpen}
                title="Give your ideas a place to live"
                detail="Save a message, write a note, or upload a file. Attach the exact revision whenever it matters."
                action="Create Resource"
                onClick={() => setModal({ type: "resource" })}
              />
            )}
          </div>
        )}
        {section === "Spaces" && (
          <div className="page">
            <PageTitle
              eyebrow="ROOM FOR EVERYTHING"
              title="Your spaces"
              description="Gather related projects, conversations and material without duplicating them."
              action="New space"
              onClick={() => setModal({ type: "space" })}
            />
            <div className="project-grid">
              {filtered(all("space")).map((s) => (
                <article className="project-card" key={s.id}>
                  <span
                    className="space-icon"
                    style={{ background: s.data.color }}
                  >
                    <Layers size={21} />
                  </span>
                  <h2>{s.data.name}</h2>
                  <p>{s.data.description}</p>
                  {all("project")
                    .filter((p) => p.data.spaceId === s.id)
                    .map((p) => (
                      <button
                        className="inline-link"
                        key={p.id}
                        onClick={() => go("Projects", p.id)}
                      >
                        <FolderOpen size={15} />
                        {p.data.name}
                        <ArrowRight size={14} />
                      </button>
                    ))}
                  <button
                    className="quiet"
                    onClick={() => setModal({ type: "project", spaceId: s.id })}
                  >
                    <Plus size={14} /> Add project
                  </button>
                </article>
              ))}
            </div>
            {!all("space").length && (
              <Empty
                icon={Layers}
                title="Make a little room"
                detail="A space can hold a side project, an area of life, or a long-term ambition."
                action="Create space"
                onClick={() => setModal({ type: "space" })}
              />
            )}
          </div>
        )}
        {section === "Projects" && (
          <div className="page project-page">
            {selected && get(selected)?.kind === "project" ? (
              <ProjectView
                project={get(selected)!}
                rows={state.records}
                command={command}
                setPanel={setPanel}
                openModal={setModal}
                back={() => setSelected("")}
                search={search}
              />
            ) : (
              <>
                <PageTitle
                  eyebrow="FROM IDEA TO DONE"
                  title="Work with a direction"
                  description="Shape the outcome, map the steps, and let your agents help you move forward."
                  action="New project"
                  onClick={() => setModal({ type: "project" })}
                />
                <div className="project-grid">
                  {filtered(all("project")).map((p) => {
                    const tasks = all("task").filter(
                        (t) => t.data.projectId === p.id,
                      ),
                      done = tasks.filter(
                        (t) => t.data.status === "accepted",
                      ).length;
                    return (
                      <button
                        className="project-card"
                        key={p.id}
                        onClick={() => setSelected(p.id)}
                      >
                        <FolderOpen size={24} />
                        <h2>{p.data.name}</h2>
                        <p>
                          {p.data.description || "An idea worth making real."}
                        </p>
                        <div className="progress-track">
                          <span
                            style={{
                              width: tasks.length
                                ? `${(done / tasks.length) * 100}%`
                                : "0%",
                            }}
                          />
                        </div>
                        <footer>
                          {done} of {tasks.length} accepted
                          <ArrowRight size={16} />
                        </footer>
                      </button>
                    );
                  })}
                </div>
                <div className="outline-toolbar">
                  <h3 className="subheading">Loose tasks</h3>
                  {all("task").some((t) => !t.data.projectId) && (
                    <button
                      className="secondary"
                      onClick={() => setModal({ type: "scope" })}
                    >
                      <Play size={14} /> Work session
                    </button>
                  )}
                </div>
                {all("task")
                  .filter((t) => !t.data.projectId)
                  .map((t) => (
                    <TaskRow key={t.id} task={t} onClick={() => setPanel(t)} />
                  ))}
                {!all("project").length && !all("task").length && (
                  <Empty
                    icon={FolderOpen}
                    title="Start with an outcome"
                    detail="You don’t need the whole plan yet. Create a project and work out the details with an agent."
                    action="Create project"
                    onClick={() => setModal({ type: "project" })}
                  />
                )}
              </>
            )}
          </div>
        )}
        {section === "Agents" && (
          <div className="page">
            <PageTitle
              eyebrow="YOUR COLLABORATORS"
              title="Different perspectives. One workspace."
              description="Each agent has its own conversation context. Material is shared explicitly."
              action="New agent"
              onClick={() => setModal({ type: "agent" })}
            />
            <div className="project-grid">
              {all("agent").map((a) => (
                <article className="agent-card" key={a.id}>
                  <span
                    className="avatar large"
                    style={{ background: a.data.color, color: "white" }}
                  >
                    {a.data.name[0]}
                  </span>
                  <h2>{a.data.name}</h2>
                  <p>{a.data.role}</p>
                  <div className="tags">
                    {a.data.capabilities.map((c: string) => (
                      <span key={c}>{c}</span>
                    ))}
                  </div>
                  <small>
                    {a.data.enabled
                      ? "Codex · isolated workspace tools"
                      : "Paused · no new responses"}
                  </small>
                  <button
                    className="quiet"
                    onClick={() => setModal({ type: "agent", edit: a })}
                  >
                    <Settings size={14} /> Edit identity
                  </button>
                  <button
                    className="secondary"
                    disabled={!a.data.enabled}
                    onClick={() => {
                      const c = all("conversation").find(
                        (c) =>
                          c.data.kind === "dm" && c.data.agentIds[0] === a.id,
                      );
                      if (c) go("Messages", c.id);
                      else
                        command("conversation.create", {
                          title: a.data.name,
                          agentIds: [a.id],
                        })
                          .then((c) => go("Messages", c.id))
                          .catch(() => {});
                    }}
                  >
                    Message {a.data.name}
                    <ArrowRight size={16} />
                  </button>
                </article>
              ))}
            </div>
            <p className="muted">
              AgentOS / Hermes can be added through the connector contract. They
              are not connected in this build.
            </p>
          </div>
        )}
      </main>
      {panel && (
        <DetailPanel
          row={{
            ...(get(panel.id) || panel),
            viewRevisionId: panel.viewRevisionId,
          }}
          rows={state.records}
          command={command}
          close={() => setPanel(null)}
          open={setPanel}
          name={name}
          auth={auth}
          setNotice={setNotice}
        />
      )}
      {modal && (
        <Modal
          title={
            (
              {
                conversation: "Start a conversation",
                project: "New project",
                space: "New space",
                resource: "New Resource",
                task: "Capture a task",
                settings: "Workspace settings",
                scope: "Review execution scope",
                forward: "Forward message",
                link: "Link a pull request",
                plan: "Plan with an agent",
                agent: "Agent identity",
              } as Record<string, string>
            )[modal.type] || "Workspace"
          }
          close={() => setModal(null)}
        >
          <ModalContent
            onNavigate={go}
            modal={modal}
            rows={state.records}
            command={command}
            done={modalDone}
            close={() => setModal(null)}
            setPanel={setPanel}
            state={state}
            auth={auth}
            fetcher={fetcher}
            refresh={refresh}
            setAuth={setAuth}
            setNotice={setNotice}
          />
        </Modal>
      )}
    </div>
  );
}
function PageTitle({ eyebrow, title, description, action, onClick }: any) {
  return (
    <div className="page-title">
      <div>
        <p className="eyebrow">{eyebrow}</p>
        <h1>{title}</h1>
        <p>{description}</p>
      </div>
      {action && (
        <button className="primary" onClick={onClick}>
          <Plus size={16} />
          {action}
        </button>
      )}
    </div>
  );
}
function Empty({ icon: Icon, title, detail, action, onClick }: any) {
  return (
    <div className="empty">
      <span>
        <Icon size={29} />
      </span>
      <h2>{title}</h2>
      <p>{detail}</p>
      {action && (
        <button className="secondary" onClick={onClick}>
          {action}
          <ArrowRight size={15} />
        </button>
      )}
    </div>
  );
}
function Modal({ title, close, children }: any) {
  const root = useRef<HTMLElement>(null);
  useEffect(() => {
    const prior = document.activeElement as HTMLElement;
    const dialog = root.current;
    const keyboard = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        close();
      }
      if (event.key === "Tab" && dialog) {
        const focusable = Array.from(
          dialog.querySelectorAll<HTMLElement>(
            "button:not([disabled]), input:not([disabled]), textarea:not([disabled]), select:not([disabled]), a[href], summary",
          ),
        ).filter((el) => el.offsetParent !== null);
        const first = focusable[0],
          last = focusable.at(-1);
        if (event.shiftKey && document.activeElement === first) {
          event.preventDefault();
          last?.focus();
        } else if (!event.shiftKey && document.activeElement === last) {
          event.preventDefault();
          first?.focus();
        }
      }
    };
    document.addEventListener("keydown", keyboard);
    if (dialog && !dialog.contains(document.activeElement)) {
      (
        (dialog.querySelector("input, textarea, select") as HTMLElement) ||
        dialog.querySelector("button")
      )?.focus();
    }
    return () => {
      document.removeEventListener("keydown", keyboard);
      prior?.focus();
    };
  }, []);
  return (
    <div
      className="modal-backdrop"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) close();
      }}
    >
      <section
        ref={root}
        className="modal"
        role="dialog"
        aria-modal="true"
        aria-label={title}
      >
        <header>
          <h2>{title}</h2>
          <button aria-label="Close dialog" onClick={close}>
            <X size={19} />
          </button>
        </header>
        {children}
      </section>
    </div>
  );
}
function Chat({
  conversation: c,
  rows,
  command,
  name,
  get,
  showResource,
  busy,
  openModal,
  setPanel,
}: any) {
  const [draft, setDraft] = useState(
      localStorage.getItem(`draft:${c.id}`) || "",
    ),
    [recipient, setRecipient] = useState("auto"),
    [mode, setMode] = useState("auto"),
    [reply, setReply] = useState<Row | null>(null),
    [correction, setCorrection] = useState<Row | null>(null),
    [pins, setPins] = useState<Attachment[]>([]),
    [attach, setAttach] = useState(false);
  const end = useRef<HTMLDivElement>(null),
    scroller = useRef<HTMLDivElement>(null),
    nearBottom = useRef(true);
  const messages = rows.filter(
    (r: Row) => r.kind === "message" && r.data.conversationId === c.id,
  );
  const requests = rows.filter(
    (r: Row) =>
      r.kind === "request" &&
      r.data.conversationId === c.id &&
      ["pending", "claimed", "running", "needs_reconciliation"].includes(
        r.data.status,
      ),
  );
  useEffect(() => {
    localStorage.setItem(`draft:${c.id}`, draft);
  }, [draft, c.id]);
  useEffect(() => {
    if (nearBottom.current) end.current?.scrollIntoView({ behavior: "smooth" });
  }, [messages.length, requests.length]);
  async function send(e: React.FormEvent) {
    e.preventDefault();
    if (!draft.trim()) return;
    try {
      await command("message.send", {
        conversationId: c.id,
        body: draft,
        mode: recipient === "everyone" && mode === "auto" ? "discuss" : mode,
        recipientIds: !["auto", "everyone"].includes(recipient)
          ? [recipient]
          : undefined,
        replyTo: reply?.id,
        corrects: correction?.id,
        attachments: pins,
      });
      setDraft("");
      setReply(null);
      setCorrection(null);
      setPins([]);
      nearBottom.current = true;
    } catch {}
  }
  return (
    <section className="chat">
      <header className="chat-header">
        <span className="avatar group-avatar">
          {c.data.kind === "dm" ? (
            name(c.data.agentIds[0])[0]
          ) : (
            <Users size={20} />
          )}
        </span>
        <div>
          <h2>{c.data.title}</h2>
          <small>
            {c.data.agentIds.map(name).join(" · ")}
            <span className="sep">/</span>Private conversation
          </small>
        </div>
        <button
          className="quiet"
          title="Conversation context"
          onClick={() => setPanel(c)}
        >
          <BookOpen size={17} />
          <span>Context</span>
        </button>
      </header>
      <div
        className="message-scroll"
        ref={scroller}
        onScroll={() => {
          const el = scroller.current;
          if (el)
            nearBottom.current =
              el.scrollHeight - el.scrollTop - el.clientHeight < 140;
        }}
      >
        {!messages.length && (
          <div className="chat-welcome">
            <span className="eyebrow">A FRESH CONVERSATION</span>
            <h1>
              {c.data.kind === "dm"
                ? `What’s on your mind?`
                : "Bring a few minds together."}
            </h1>
            <p>
              {c.data.kind === "dm"
                ? `${name(c.data.agentIds[0])} is here to help you think things through and make progress.`
                : "Ask one agent, get independent opinions, or discuss an idea together."}
            </p>
            <div className="suggestions">
              {[
                "Help me turn an idea into a plan",
                "Let’s think through a decision",
                "Make a note we can return to",
              ].map((v) => (
                <button key={v} onClick={() => setDraft(v)}>
                  {v}
                  <ArrowUp size={13} />
                </button>
              ))}
            </div>
          </div>
        )}
        {messages.map((m: Row) => (
          <article
            className={`message ${m.data.authorId === "system" ? "system-message" : ""}`}
            key={m.id}
          >
            <span
              className={`avatar ${m.data.authorId === "owner" ? "owner-avatar" : ""}`}
              style={
                get(m.data.authorId)?.data.color
                  ? {
                      background: get(m.data.authorId).data.color,
                      color: "white",
                    }
                  : {}
              }
            >
              {name(m.data.authorId)[0]}
            </span>
            <div className="message-content">
              <header>
                <strong>{name(m.data.authorId)}</strong>
                <time>
                  {new Date(m.createdAt).toLocaleTimeString([], {
                    hour: "numeric",
                    minute: "2-digit",
                  })}
                </time>
                {m.data.staged && (
                  <span className="tag">Independent · awaiting others</span>
                )}
                {m.data.late && <span className="tag">Arrived after Stop</span>}
                {m.data.predatesCorrection && (
                  <span className="tag">Predates correction</span>
                )}
              </header>
              {m.data.replyTo && (
                <div className="reply-quote">
                  <CornerUpLeft size={12} />
                  {get(m.data.replyTo)?.data.body.slice(0, 160)}
                </div>
              )}
              {m.data.forwardedFrom && (
                <small className="muted">Forwarded message</small>
              )}
              <div className="message-body">
                <RichText text={m.data.body} />
              </div>
              {m.data.attachments?.map((pin: Attachment) => (
                <button
                  className="attachment"
                  key={pin.revisionId}
                  onClick={() => showResource(pin.resourceId, pin.revisionId)}
                >
                  <FileText size={17} />
                  {get(pin.resourceId)?.data.title}
                  <small>v{get(pin.revisionId)?.data.number}</small>
                  {get(pin.resourceId)?.data.latestRevisionId !==
                    pin.revisionId && <small>Newer available</small>}
                </button>
              ))}
              <div className="message-actions">
                {m.data.authorId === "owner" && (
                  <button
                    title="Correct message"
                    onClick={() => {
                      setCorrection(m);
                      setDraft(m.data.body);
                      setReply(null);
                    }}
                  >
                    <RefreshCw size={14} />
                  </button>
                )}
                <button title="Reply" onClick={() => setReply(m)}>
                  <CornerUpLeft size={14} />
                </button>
                <button
                  title="Save as Resource"
                  onClick={() =>
                    openModal({
                      type: "resource",
                      text: m.data.body,
                      conversationId: c.id,
                    })
                  }
                >
                  <BookOpen size={14} />
                </button>
                <button
                  title="Forward"
                  onClick={() => openModal({ type: "forward", message: m })}
                >
                  <Forward size={14} />
                </button>
                {m.data.requestId && (
                  <button
                    title="Inspect response context"
                    onClick={() => {
                      const r = rows.find(
                        (r: Row) =>
                          r.kind === "run" &&
                          r.data.requestId === m.data.requestId,
                      );
                      if (r) setPanel(r);
                    }}
                  >
                    <MoreHorizontal size={15} />
                  </button>
                )}
              </div>
            </div>
          </article>
        ))}
        {requests.map((r: Row) => (
          <div className="request-status" key={r.id}>
            <span className={r.data.status === "running" ? "pulsing" : ""} />
            <strong>{name(r.data.agentId)}</strong>
            <span>
              {r.data.status === "pending"
                ? "Waiting to respond"
                : r.data.status === "needs_reconciliation"
                  ? "Needs reconciliation"
                  : "Working on this"}
            </span>
            <button
              onClick={() => {
                const run = get(r.data.runId);
                if (run) setPanel(run);
              }}
            >
              Details
            </button>
          </div>
        ))}
        {rows
          .filter(
            (r: Row) =>
              r.kind === "request" &&
              r.data.conversationId === c.id &&
              ["failed", "cancelled", "superseded"].includes(r.data.status),
          )
          .slice(-5)
          .map((r: Row) => (
            <div
              className={`request-status ${r.data.status === "failed" ? "failed" : ""}`}
              key={r.id}
            >
              <span />
              <div className="request-error">
                <strong>{name(r.data.agentId)}</strong> · {r.data.status}
                {r.data.error && <p>{r.data.error}</p>}
                {r.data.status === "failed" && (
                  <small>
                    Your message is saved. Send a new message when you want
                    another attempt.
                  </small>
                )}
              </div>
              {r.data.runId && (
                <button
                  onClick={() => {
                    const run = get(r.data.runId);
                    if (run) setPanel(run);
                  }}
                >
                  Inspect attempt
                </button>
              )}
            </div>
          ))}
        <div ref={end} />
      </div>
      <div className="composer-area">
        {c.data.stopped ? (
          <div className="stopped">
            <Pause size={18} />
            <span>This conversation is stopped.</span>
            <button
              className="secondary"
              onClick={() =>
                command("conversation.continue", { id: c.id }).catch(() => {})
              }
            >
              Continue
            </button>
          </div>
        ) : (
          <form className="composer" onSubmit={send}>
            {correction && (
              <div className="reply-preview">
                Correcting your earlier message
                <button
                  type="button"
                  aria-label="Cancel correction"
                  onClick={() => setCorrection(null)}
                >
                  <X size={14} />
                </button>
              </div>
            )}
            {reply && (
              <div className="reply-preview">
                Replying to {name(reply.data.authorId)}:{" "}
                {reply.data.body.slice(0, 80)}
                <button
                  type="button"
                  aria-label="Cancel reply"
                  onClick={() => setReply(null)}
                >
                  <X size={14} />
                </button>
              </div>
            )}
            {pins.length > 0 && (
              <div className="pin-list">
                {pins.map((p) => (
                  <button
                    type="button"
                    className="tag"
                    key={p.revisionId}
                    onClick={() => setPins(pins.filter((x) => x !== p))}
                  >
                    {get(p.resourceId)?.data.title}
                    <X size={12} />
                  </button>
                ))}
              </div>
            )}
            <textarea
              aria-label="Message"
              placeholder={`Message ${c.data.kind === "dm" ? name(c.data.agentIds[0]) : "the group"}…`}
              value={draft}
              onChange={(e) => setDraft(e.target.value)}
              onKeyDown={(e) => {
                if (
                  e.key === "Enter" &&
                  !e.shiftKey &&
                  !e.nativeEvent.isComposing
                ) {
                  e.preventDefault();
                  e.currentTarget.form?.requestSubmit();
                }
              }}
            />
            <div className="composer-toolbar">
              <button
                type="button"
                title="Attach Resource"
                onClick={() => setAttach(!attach)}
              >
                <Paperclip size={18} />
              </button>
              <select
                aria-label="Recipient"
                value={recipient}
                onChange={(e) => setRecipient(e.target.value)}
              >
                <option value="auto">
                  Auto
                  {c.data.focusAgentId ? ` · ${name(c.data.focusAgentId)}` : ""}
                </option>
                <option value="everyone">Everyone</option>
                {c.data.agentIds.map((id: string) => (
                  <option key={id} value={id}>
                    @{name(id)}
                  </option>
                ))}
              </select>
              {c.data.kind !== "dm" && (
                <select
                  aria-label="Participation mode"
                  value={mode}
                  onChange={(e) => {
                    setMode(e.target.value);
                    if (e.target.value !== "auto") setRecipient("everyone");
                  }}
                >
                  <option value="auto">Conversation</option>
                  <option value="independent">Independent opinions</option>
                  <option value="discuss">Discuss together</option>
                </select>
              )}
              <span className="spacer" />
              {
                <button
                  type="button"
                  className="stop-button"
                  onClick={() =>
                    command("conversation.stop", { id: c.id }).catch(() => {})
                  }
                >
                  <Square size={12} />
                  Stop
                </button>
              }
              <button
                type="submit"
                className="send"
                aria-label="Send message"
                disabled={!draft.trim() || busy}
              >
                <ArrowUp size={19} />
              </button>
            </div>
            {attach && (
              <div className="attach-menu">
                {rows
                  .filter((r: Row) => r.kind === "resource")
                  .map((r: Row) => {
                    const shared =
                      r.data.conversationIds.includes(c.id) &&
                      c.data.agentIds.every((id: string) =>
                        r.data.agentIds.includes(id),
                      );
                    return (
                      <div className="attach-option" key={r.id}>
                        <FileText size={14} />
                        <span>
                          {r.data.title}
                          <small>
                            {r.data.stagedFor
                              ? "Private until its independent round finishes"
                              : `Revision ${get(r.data.latestRevisionId)?.data.number} · ${shared ? "Shared here" : "Access required"}`}
                          </small>
                        </span>
                        <button
                          type="button"
                          disabled={!!r.data.stagedFor || busy}
                          onClick={async () => {
                            try {
                              if (!shared)
                                await command("resource.grant", {
                                  id: r.id,
                                  expectedRevision: r.revision,
                                  conversationIds: [
                                    ...new Set([
                                      ...r.data.conversationIds,
                                      c.id,
                                    ]),
                                  ],
                                  agentIds: [
                                    ...new Set([
                                      ...r.data.agentIds,
                                      ...c.data.agentIds,
                                    ]),
                                  ],
                                  projectIds: r.data.projectIds,
                                  spaceIds: r.data.spaceIds,
                                });
                              setPins([
                                ...pins.filter((p) => p.resourceId !== r.id),
                                {
                                  resourceId: r.id,
                                  revisionId: r.data.latestRevisionId,
                                },
                              ]);
                              setAttach(false);
                            } catch {}
                          }}
                        >
                          {shared ? "Attach" : "Share & attach"}
                        </button>
                      </div>
                    );
                  })}
                {!rows.some((r: Row) => r.kind === "resource") && (
                  <p>Create a Resource first.</p>
                )}
              </div>
            )}
          </form>
        )}
        <small className="composer-footnote">
          {c.data.kind === "dm"
            ? "Only this conversation’s context is shared."
            : "Auto follows your addressee and the current exchange. You can always choose."}{" "}
          Enter to send · Shift + Enter for a new line
        </small>
      </div>
    </section>
  );
}

createRoot(document.getElementById("root")!).render(<App />);
