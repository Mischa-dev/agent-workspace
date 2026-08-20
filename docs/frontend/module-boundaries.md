# Frontend module boundaries

## Application and shared infrastructure

- `app/` owns providers, route composition, lazy route boundaries, and the root
  rendering error boundary.
- `auth/` owns session normalization, authentication gates, and public auth UI.
- `shell/` owns the responsive global/Project navigation and appearance entry
  point; it does not own feature queries.
- `theme/` owns safe presentation preference parsing and DOM token application.
- `ui/` owns dependency-light interaction primitives and generic async states;
  it must not contain Project policy.
- `realtime/` owns bounded socket lifecycle, invalidation, and ephemeral stream
  state. Durable data remains in TanStack Query.

## Feature ownership

Feature folders own API adapters, queries, mutations, screens, and their
business-specific components. Query keys begin with the Project boundary for
Project-scoped records. Capabilities stay server-returned and fail closed.

The first stable decomposition seams are:

- `ThreadPage` orchestration versus `ThreadPresentation` timeline, run,
  approval, skeleton, and context presentation
- `WorkItemsPage` filters/list/create flow versus `WorkItemBoard` projection
- `RealtimeProvider` context publication versus `useRealtimeConnection`
- `AppShell` modal-region state versus `ProjectRail` and `ProjectSidebar`

## Migration rule

The retained legacy stylesheet remains a temporary feature-family seam while
tokens, base, primitives, shell, and new feature rules live in separate files.
Old selectors may coexist only in that seam. There is no second router,
alternate application tree, duplicated task model, or competing realtime
authority.
