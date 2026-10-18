import { Icon } from '../ui'
import { pausedUntilLabel, useNotificationPause } from './notificationPause'
import './pausedMark.css'

/** A moon on your avatar while notifications are paused. */
export default function PausedMark() {
  const { until } = useNotificationPause()
  if (!until) return null
  return (
    <span className="aw-rail__paused" title={`Notifications paused until ${pausedUntilLabel(until, new Date())}`}>
      <Icon name="moon" size={11} />
      <span className="visually-hidden">Notifications paused</span>
    </span>
  )
}
