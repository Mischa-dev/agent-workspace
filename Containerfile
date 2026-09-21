FROM docker.io/library/node:24.18.1-bookworm-slim AS build
WORKDIR /workspace
COPY package.json pnpm-lock.yaml pnpm-workspace.yaml tsconfig.base.json ./
COPY apps ./apps
COPY packages ./packages
RUN npm exec --yes pnpm@11.19.0 -- install --frozen-lockfile
RUN npm exec --yes pnpm@11.19.0 -- build

FROM docker.io/library/node:24.18.1-bookworm-slim
WORKDIR /workspace
COPY --from=build --chown=node:node /workspace /workspace
ENV NODE_ENV=production BIND_HOST=0.0.0.0 PORT=4310 DATA_DIR=/var/lib/workspace WORKSPACE_ROOT=/workspace
USER node
VOLUME /var/lib/workspace
EXPOSE 4310
HEALTHCHECK --interval=30s --timeout=5s --start-period=30s CMD ["node", "dist/apps/server/src/healthcheck.js"]
CMD ["node", "dist/apps/server/src/server.js"]
