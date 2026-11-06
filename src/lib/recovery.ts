/** Aviso após recarregar a interface depois de uma falha de render: a marca vai na sessionStorage e some ao ser lida. */
const KEY = "regente.recarregadoAposErro";

type Store = Pick<Storage, "getItem" | "setItem" | "removeItem">;

/** A sessionStorage pode faltar (janela privada, dados bloqueados): sem ela, o aviso simplesmente não aparece. */
function defaultStore(): Store | undefined {
  try {
    return typeof sessionStorage === "undefined" ? undefined : sessionStorage;
  } catch {
    return undefined;
  }
}

/** Marca que a próxima carga da interface é um recarregamento feito pelo usuário após uma falha. */
export function markRecovery(store: Store | undefined = defaultStore()): void {
  try {
    store?.setItem(KEY, "1");
  } catch {
    // sem a marca só falta o aviso
  }
}

/** True uma vez após `markRecovery`; a marca é apagada na leitura. */
export function takeRecovery(store: Store | undefined = defaultStore()): boolean {
  try {
    if (store?.getItem(KEY) !== "1") return false;
    store.removeItem(KEY);
    return true;
  } catch {
    return false;
  }
}
