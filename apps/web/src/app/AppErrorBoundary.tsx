import { Component, type ErrorInfo, type ReactNode } from 'react'

import { reportError } from '../telemetry/reportErrors'
import { Button } from '../ui'
import { failingComponent } from './failingComponent'

type Props = { children: ReactNode }
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
          </div>
        </main>
      )
    }
    return this.props.children
  }
}
