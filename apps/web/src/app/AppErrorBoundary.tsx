import { Component, type ErrorInfo, type ReactNode } from 'react'

type Props = { children: ReactNode }
type State = { failed: boolean }

export class AppErrorBoundary extends Component<Props, State> {
  state: State = { failed: false }

  static getDerivedStateFromError(): State {
    return { failed: true }
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    // Runtime diagnostics remain in the browser console; no error payload or
    // server data is persisted by this recovery boundary.
    void error
    void info
  }

  render() {
    if (this.state.failed) {
      return (
        <main className="session-state">
          <div className="error-state" role="alert">
            <h1>Workspace needs a clean reload</h1>
            <p>
              The current view could not be rendered. Durable Project data and
              hosted work remain on the server.
            </p>
            <button onClick={() => window.location.reload()} type="button">
              Reload workspace
            </button>
          </div>
        </main>
      )
    }
    return this.props.children
  }
}
