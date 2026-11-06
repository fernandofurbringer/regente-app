import { Component, type ErrorInfo, type ReactNode } from "react";
import { api } from "../lib/ipc";
import { markRecovery } from "../lib/recovery";

type Props = { children: ReactNode };
type State = { error: Error | null; zip: string | null; diagError: string | null };

/** Última linha de defesa: uma falha de render mostra o erro em vez de uma tela preta. */
export default class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null, zip: null, diagError: null };

  static getDerivedStateFromError(error: Error): Partial<State> {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error("erro de render", error, info.componentStack);
  }

  diagnose = async () => {
    try {
      this.setState({ zip: await api.diagGenerate(), diagError: null });
    } catch (e) {
      this.setState({ diagError: e instanceof Error ? e.message : String(e) });
    }
  };

  render() {
    const { error, zip, diagError } = this.state;
    if (!error) return this.props.children;
    return (
      <div style={{ minHeight: "100vh", display: "flex", alignItems: "center", justifyContent: "center", padding: 24, background: "var(--bg)", color: "var(--fg)" }}>
        <div style={{ maxWidth: 560, display: "grid", gap: 12 }}>
          <h2 style={{ margin: 0 }}>O Regente encontrou um erro</h2>
          <pre style={{ whiteSpace: "pre-wrap", margin: 0, padding: 12, background: "var(--bg3)", border: "1px solid var(--line)", borderRadius: 6 }}>
            {error.message}
          </pre>
          {zip && <div>Diagnóstico salvo em: {zip}</div>}
          {diagError && <div style={{ color: "var(--fg2)" }}>Não foi possível gerar o diagnóstico: {diagError}</div>}
          <div style={{ display: "flex", gap: 8 }}>
            <button onClick={this.diagnose} style={{ background: "var(--accent-btn)", color: "#fff", border: 0, borderRadius: 6, padding: "6px 12px", cursor: "pointer" }}>
              Gerar diagnóstico
            </button>
            <button onClick={() => { markRecovery(); location.reload(); }} style={{ background: "var(--bg3)", color: "var(--fg)", border: "1px solid var(--line)", borderRadius: 6, padding: "6px 12px", cursor: "pointer" }}>
              Recarregar
            </button>
          </div>
        </div>
      </div>
    );
  }
}
