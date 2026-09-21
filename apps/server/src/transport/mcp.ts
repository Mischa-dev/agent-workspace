import { createMcpHandler, McpServer } from "@modelcontextprotocol/server";
import { toNodeHandler } from "@modelcontextprotocol/node";
import { z } from "zod";
import type { FastifyInstance } from "fastify";
import type { Actor } from "../domain/policy.js";
import {
  AGENT_OPERATIONS,
  authenticatedParticipant,
  boundActor,
  participantCommand,
  type AgentTransportDependencies,
} from "./agent-http.js";
import { MCP_INPUTS, MCP_DESCRIPTIONS } from "./mcp-inputs.js";

export async function registerMcp(
  app: FastifyInstance,
  deps: AgentTransportDependencies,
  publicOrigin: string,
) {
  const contextual = new Set([
    "request.renew",
    "request.release",
    "message.reply",
    "result.submit",
    "request.handoff",
  ]);
  const handler = createMcpHandler(
    ({ authInfo }) => {
      const actor = authInfo?.extra?.actor as Actor | undefined;
      if (!actor?.participant || actor.owner)
        throw new Error("Participant identity required");
      const server = new McpServer({
        name: "agent-workspace",
        version: "0.1.0",
      });
      for (const operation of AGENT_OPERATIONS)
        server.registerTool(
          operation.replaceAll(".", "_"),
          {
            description: `${MCP_DESCRIPTIONS[operation] || `Run checked workspace operation ${operation}.`} Use a stable commandId and retain it when retrying the same action. Identity and access come from the credential.`,
            inputSchema: z.object({
              commandId: z
                .string()
                .min(1)
                .max(160)
                .describe(
                  "Stable ID for this action; persist before making the call and reuse only with identical input.",
                ),
              input: MCP_INPUTS[operation],
              requestId: (contextual.has(operation)
                ? z.string().uuid()
                : z.string().uuid().optional()
              ).describe(
                "Claimed request ID. Required for renew, release, reply, result and handoff; freezes access to the claimed context.",
              ),
            }),
            annotations: {
              readOnlyHint:
                operation.endsWith(".list") ||
                operation.endsWith(".read") ||
                operation === "agent.me",
              destructiveHint: false,
              openWorldHint: false,
            },
          },
          async ({ commandId, input, requestId }) => {
            try {
              const command = participantCommand({
                id: commandId,
                type: operation,
                input,
                requestId,
              });
              const result = await deps.execute(
                boundActor(actor, command.requestId),
                command,
              );
              return {
                content: [
                  { type: "text" as const, text: JSON.stringify(result) },
                ],
              };
            } catch (error) {
              const e = error as any;
              return {
                isError: true,
                content: [
                  {
                    type: "text" as const,
                    text:
                      e.status && e.status < 500
                        ? e.message
                        : "The workspace command failed.",
                  },
                ],
              };
            }
          },
        );
      return server;
    },
    { responseMode: "json" },
  );
  const node = toNodeHandler(handler, {
    onerror: () => app.log.error("MCP request failed"),
  });
  app.route({
    url: "/mcp",
    method: ["GET", "POST", "DELETE"],
    handler: async (req, reply) => {
      let actor: Actor;
      try {
        actor = await authenticatedParticipant(req, deps);
      } catch (error) {
        reply.header(
          "WWW-Authenticate",
          `Bearer resource_metadata="${publicOrigin}/.well-known/oauth-protected-resource/mcp"`,
        );
        throw error;
      }
      (req.raw as any).auth = {
        token: "verified",
        clientId: actor.id,
        scopes: ["workspace:participate"],
        resource: new URL(`${publicOrigin}/mcp`),
        extra: { actor },
      };
      for (const [key, value] of Object.entries(reply.getHeaders()))
        if (value !== undefined) reply.raw.setHeader(key, value);
      reply.hijack();
      await node(req.raw, reply.raw, req.body);
    },
  });
  app.addHook("onClose", async () => handler.close());
}
