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
        <main className="aw-boot">
          <div role="alert">
            <h1>Something went wrong on this page</h1>
            <p>Reload to get back to work. Your messages and running work are safe on the server.</p>
            <button className="aw-btn aw-btn--primary aw-btn--md" onClick={() => window.location.reload()} type="button">
              Reload workspace
            </button>
          </div>
        </main>
      )
    }
    return this.props.children
  }
}
