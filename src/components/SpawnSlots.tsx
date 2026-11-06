import { useShallow } from "zustand/react/shallow";
import { api, type SlotState } from "../lib/ipc";
import { useStore } from "../state/store";

const LABEL: Record<SlotState, string> = { na_fila: "Na fila", abrindo: "Abrindo", aguardando: "Aguardando CLI", ok: "Aberto", falha: "Falha ao abrir", cancelado: "Cancelado", done: "" };

export default function SpawnSlots({ wsId }: { wsId: string }) {
  const missions = useStore(useShallow((s) => (s.missions[wsId] ?? []).filter((m) => s.slots[m.id])));
  const slots = useStore((s) => s.slots);
  if (!missions.length) return null;
  return (
    <div className="slots" aria-live="polite">
      {missions.flatMap((m) =>
        Object.entries(slots[m.id] ?? {})
          .filter(([, s]) => s.state !== "ok")
          .map(([i, s]) => (
            <span key={`${m.id}-${i}`} className="slot" data-state={s.state}>
              {m.name} #{Number(i) + 1}: {LABEL[s.state]}{s.reason ? `: ${s.reason}` : ""}
              {s.state === "na_fila" && <button className="icon-btn" aria-label={`Cancelar #${Number(i) + 1}`} onClick={() => api.spawnCancel(m.id, Number(i))}>✕</button>}
            </span>
          )),
      )}
    </div>
  );
}
