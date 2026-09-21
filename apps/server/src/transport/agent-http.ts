import type { FastifyInstance, FastifyRequest } from "fastify";
import { createHash } from "node:crypto";
import type { Actor } from "../domain/policy.js";
import type { Command } from "../../../../packages/shared/src/index.js";
import { AppError } from "../domain/store.js";
import type { Storage } from "../storage/local.js";

export const AGENT_OPERATIONS = [
  "agent.me",
  "agent.list",
  "inbox.list",
  "request.claim",
  "request.renew",
  "request.release",
  "message.reply",
  "message.send",
  "request.handoff",
  "conversation.list",
  "conversation.read",
  "space.list",
  "project.list",
  "resource.list",
  "resource.read",
  "resource.create",
  "resource.revise",
  "task.list",
  "task.create",
  "task.update",
  "plan.propose",
  "result.submit",
] as const;

export interface AgentTransportDependencies {
  authenticate(token: string): Promise<Actor>;
  execute(actor: Actor, command: Command): Promise<unknown>;
  storage: Storage;
  upload(
    actor: Actor,
    command: Command,
    metadata: {
      storageKey: string;
      size: number;
      checksum: string;
      filename: string;
      mimeType: string;
    },
  ): Promise<unknown>;
  download(
    actor: Actor,
    resourceId: string,
    revisionId: string,
  ): Promise<{ storageKey: string; checksum: string; filename?: string }>;
}

export function bearerToken(req: FastifyRequest): string {
  const match = /^Bearer ([A-Za-z0-9._~-]{20,4096})$/i.exec(
    req.headers.authorization || "",
  );
  if (!match)
    throw new AppError(401, "A valid agent bearer credential is required.");
  return match[1];
}

export async function authenticatedParticipant(
  req: FastifyRequest,
  deps: Pick<AgentTransportDependencies, "authenticate">,
): Promise<Actor> {
  const actor = await deps.authenticate(bearerToken(req));
  if (!actor.participant || actor.owner)
    throw new AppError(403, "Participant identity required.");
  return actor;
}

export function boundActor(actor: Actor, requestId?: unknown): Actor {
  if (requestId === undefined) return actor;
  if (
    typeof requestId !== "string" ||
    !/^[0-9a-f]{8}-[0-9a-f-]{27}$/i.test(requestId)
  )
    throw new AppError(400, "Invalid request ID.");
  return { ...actor, requestId };
}

export function participantCommand(
  value: unknown,
): Command & { requestId?: string } {
  const command = value as any;
  if (
    !command ||
    typeof command !== "object" ||
    Array.isArray(command) ||
    typeof command.id !== "string" ||
    command.id.length < 1 ||
    command.id.length > 160 ||
    !AGENT_OPERATIONS.includes(command.type) ||
    !command.input ||
    typeof command.input !== "object" ||
    Array.isArray(command.input)
  )
    throw new AppError(
      400,
      "A supported command, stable id and object input are required.",
    );
  if (
    command.input.storageKey ||
    command.input.trustedUpload ||
    command.input.owner ||
    command.actor
  )
    throw new AppError(
      400,
      "Transport authority and file storage keys cannot be supplied in commands.",
    );
  return {
    id: command.id,
    type: command.type,
    input: command.input,
    ...(command.requestId !== undefined
      ? { requestId: command.requestId }
      : {}),
  };
}

export async function registerAgentHttp(
  app: FastifyInstance,
  deps: AgentTransportDependencies,
) {
  app.post("/api/agent/commands", async (req) => {
    const actor = await authenticatedParticipant(req, deps),
      command = participantCommand(req.body);
    return deps.execute(boundActor(actor, command.requestId), command);
  });
  app.post("/api/agent/uploads", async (req) => {
    const actor = await authenticatedParticipant(req, deps);
    const part = await req.file();
    if (!part) throw new AppError(400, "Choose a file.");
    const bytes = await part.toBuffer();
    if (part.file.truncated)
      throw new AppError(413, "Files are limited to 10 MB.");
    const field = (name: string) => {
      const value = part.fields[name] as any;
      return typeof value?.value === "string" ? value.value : undefined;
    };
    const id = field("commandId");
    if (!id || id.length > 160)
      throw new AppError(400, "A stable commandId is required.");
    let input: Record<string, unknown>;
    try {
      input = JSON.parse(field("input") || "{}");
      if (!input || typeof input !== "object" || Array.isArray(input))
        throw new Error();
    } catch {
      throw new AppError(400, "Upload input must be a JSON object.");
    }
    const command = participantCommand({
      id,
      type: "resource.create",
      input: { ...input, title: input.title || part.filename },
      requestId: field("requestId"),
    });
    const scoped = boundActor(actor, command.requestId);
    const saved = await deps.storage.put(bytes);
    return deps.upload(scoped, command, {
      storageKey: saved.key,
      size: saved.size,
      checksum: saved.checksum,
      filename: part.filename,
      mimeType: part.mimetype,
    });
  });
  app.get(
    "/api/agent/resources/:id/revisions/:revisionId/download",
    async (req, reply) => {
      const actor = boundActor(
        await authenticatedParticipant(req, deps),
        (req.query as any).requestId,
      );
      const { id, revisionId } = req.params as any;
      const metadata = await deps.download(actor, id, revisionId);
      const bytes = await deps.storage.get(metadata.storageKey);
      if (
        createHash("sha256").update(bytes).digest("hex") !== metadata.checksum
      )
        throw new AppError(409, "File checksum mismatch.");
      reply
        .type("application/octet-stream")
        .header(
          "Content-Disposition",
          `attachment; filename*=UTF-8''${encodeURIComponent(metadata.filename || "download")}`,
        )
        .header("Cache-Control", "no-store");
      return bytes;
    },
  );
}
