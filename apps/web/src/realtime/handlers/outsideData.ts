import { dashboards, opAnswers, type Handler } from './touches'

// Outside data's events (T1.34): a saved query changed, or a run of it
// stored its answer; a connection changed. The query lists and reads, and
// dashboards (whose query tiles show the answers), go stale.

const queryChanged: Handler = () => [opAnswers('data_query_list'), opAnswers('data_query_get'), dashboards]

const connectionChanged: Handler = () => [opAnswers('data_connection_list'), opAnswers('data_query_list'), opAnswers('data_query_get')]

export const outsideDataHandlers: Record<string, Handler> = {
  'data_query.changed': queryChanged,
  'data_connection.changed': connectionChanged,
}
