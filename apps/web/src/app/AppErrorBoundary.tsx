import { Component, type ErrorInfo, type ReactNode } from 'react'

import { reportError } from '../telemetry/reportErrors'
import { Button } from '../ui'
import { failingComponent } from './failingComponent'

type Props = {
  children: ReactNode
  /** The page's address: going somewhere else clears the error, so Back and links work again. */
  resetKey?: string
}
type State = { failed: boolean }

export class AppErrorBoundary extends Component<Props, State> {
  state: State = { failed: false }

  static getDerivedStateFromError(): State {
    return { failed: true }
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    // Only what reportError allows leaves the browser: the message, the
    // component that threw and the route path. No props, state or bodies.
    const component = failingComponent(info.componentStack)
    reportError(error.message || error.name, component ? `react:${component}` : 'react')
  }

  componentDidUpdate(previous: Props) {
    if (this.state.failed && previous.resetKey !== this.props.resetKey) this.setState({ failed: false })
  }

  private readonly retry = () => {
    this.setState({ failed: false })
  }

  render() {
    if (this.state.failed) {
      return (
        <main className="aw-boot">
          <div role="alert">
            <h1>Something went wrong on this page</h1>
            <p>Try again, or reload the workspace if it keeps happening. Your messages and running work are safe on the server.</p>
          </div>
          <div className="aw-boot__actions">
            <Button onClick={this.retry} variant="primary">
              Try again
            </Button>
            <Button onClick={() => window.location.reload()}>Reload workspace</Button>
            <Button onClick={() => window.location.assign('/home')} variant="ghost">
              Go to Home
            </Button>
          </div>
        </main>
      )
    }
    return this.props.children
  }
}
