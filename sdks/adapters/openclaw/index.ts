import { defineChannelPluginEntry } from 'openclaw/plugin-sdk/channel-core'

import { agentWorkspaceChannelPlugin } from './src/channel.ts'

export default defineChannelPluginEntry({
  id: 'agent-workspace',
  name: 'Agent Workspace',
  description: 'Agent Workspace channel: reply requests in, answers out.',
  plugin: agentWorkspaceChannelPlugin,
})
