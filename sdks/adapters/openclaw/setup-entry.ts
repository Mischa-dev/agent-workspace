import { defineSetupPluginEntry } from 'openclaw/plugin-sdk/channel-core'

import { agentWorkspaceChannelPlugin } from './src/channel.ts'

export default defineSetupPluginEntry(agentWorkspaceChannelPlugin)
