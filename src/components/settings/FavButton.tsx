import { api, errMsg } from "../../lib/ipc";
import { toggleFav } from "../../lib/favorites";
import { useStore } from "../../state/store";

/** Referência estável (um `[]` novo por leitura faz o React 19 laçar). */
const NO_FAVS: string[] = [];

/** Estrela de favorito. Salva em Configurações (campo `favorites`) e atualiza a lista na hora. */
export default function FavButton({ favKey, label }: { favKey: string; label: string }) {
  const favs = useStore((s) => s.settings?.favorites ?? NO_FAVS);
  const on = favs.includes(favKey);
  async function toggle() {
    const st = useStore.getState();
    if (!st.settings) return;
    const next = { ...st.settings, favorites: toggleFav(st.settings.favorites ?? [], favKey) };
    try {
      await api.settingsSet(next);
      st.patch({ settings: next });
    } catch (e) {
      st.toast(errMsg(e), "erro");
    }
  }
  return (
    <button type="button" className="icon-btn" aria-pressed={on} aria-label={`${on ? "Remover dos favoritos" : "Favoritar"}: ${label}`} title={on ? "Remover dos favoritos" : "Favoritar"} onClick={toggle}>
      {on ? "★" : "☆"}
    </button>
  );
}
