import { ListFilter } from 'lucide-react'

import { Download } from '../../src/files/icons'
import { Button, Icon, PageHeader, TextField } from '../../src/ui'
import { project, tasks } from './data'
import { TaskRef } from './parts/Tasks'

/*
 * The picture Muse attaches in the chat: a crop of the Tasks page with an
 * Export button beside Filter, drawn with the app's own kit. It is captured
 * once (asset.mjs) and served as a message attachment.
 */

export function ExportMock() {
  return (
    <div className="tr-mock">
      <div className="tr-mock__page">
        <div className="tr-mock__body">
          <PageHeader
            actions={
              <Button icon="plus" variant="primary">
                New task
              </Button>
            }
            icon="work"
            title="Tasks"
          >
            Everything that needs doing in {project.name}.
          </PageHeader>
          <div className="aw-worktools">
            <TextField
              aria-label="Search"
              className="aw-worktools__search"
              compact
              placeholder="Search"
              prefix={<Icon name="search" size={14} />}
              readOnly
            />
            <Button icon={ListFilter}>Filter</Button>
            <span className="tr-mock__new">
              <Button icon={Download}>Export</Button>
              <span className="tr-mock__tag">New</span>
            </span>
            <span className="aw-worktools__spacer" />
          </div>
          <ul className="aw-taskrefs">
            {[12, 26, 27, 28].map((n) => (
              <TaskRef assignee={tasks[n].who} key={n} keyLabel={`NW-${n}`} status={tasks[n].status} title={tasks[n].title} />
            ))}
          </ul>
        </div>
      </div>
    </div>
  )
}
