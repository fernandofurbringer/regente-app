//! Monitor de recursos: memória e CPU de cada painel (a árvore de processos inteira), da interface (WebView2) e do Regente.

use crate::util::lk;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Uma leitura do sistema por esse intervalo; chamadas no meio dele reaproveitam a última (CPU precisa de duas amostras).
const SAMPLE_EVERY: Duration = Duration::from_secs(2);
const WEBVIEW: &str = "msedgewebview2.exe";

/// Processo como o monitor precisa dele: cópia pura, para testar sem o sistema.
#[derive(Debug, Clone, PartialEq)]
pub struct Proc {
    pub pid: u32,
    pub parent: Option<u32>,
    /// Início do processo: um filho nunca começa antes do pai; se começou, o pid do pai foi reaproveitado.
    pub start: u64,
    pub name: String,
    pub mem: u64,
    pub cpu: f32,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Child {
    pub pid: u32,
    pub name: String,
    pub mem_mb: u64,
    pub cpu_pct: f32,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Totals {
    pub mem_mb: u64,
    pub cpu_pct: f32,
    pub procs: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PaneRes {
    pub pane_id: String,
    pub name: String,
    pub status: String,
    pub mem_mb: u64,
    pub cpu_pct: f32,
    pub procs: usize,
    pub children: Vec<Child>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub total: Totals,
    pub panes: Vec<PaneRes>,
    pub ui: Totals,
    pub app: Totals,
    /// false na primeira leitura: a CPU ainda não tem base (a interface mostra "—")
    pub cpu_ready: bool,
}

/// Descendentes de `root`, sem a raiz. Ciclo e órfão são seguros (cada pid entra uma vez, e órfão não é alcançado);
/// filho que começou antes do pai é pid reaproveitado e fica de fora.
pub fn descendants(root: u32, procs: &[Proc]) -> Vec<&Proc> {
    let by_pid: HashMap<u32, &Proc> = procs.iter().map(|p| (p.pid, p)).collect();
    let mut kids: HashMap<u32, Vec<&Proc>> = HashMap::new();
    for p in procs {
        if let Some(par) = p.parent.filter(|&x| x != p.pid) {
            kids.entry(par).or_default().push(p);
        }
    }
    let mut seen = HashSet::from([root]);
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(cur) = stack.pop() {
        let Some(me) = by_pid.get(&cur) else { continue };
        for k in kids.get(&cur).into_iter().flatten() {
            if k.start < me.start {
                continue;
            }
            if seen.insert(k.pid) {
                out.push(*k);
                stack.push(k.pid);
            }
        }
    }
    out
}

/// Raiz do painel + descendentes: memória e CPU da árvore inteira.
pub fn tree_totals<'a>(root: &'a Proc, desc: &[&'a Proc]) -> Totals {
    sum(std::iter::once(root).chain(desc.iter().copied()))
}

fn sum<'a>(it: impl Iterator<Item = &'a Proc>) -> Totals {
    let mut t = Totals::default();
    let mut bytes = 0u64;
    for p in it {
        bytes += p.mem;
        t.cpu_pct += p.cpu;
        t.procs += 1;
    }
    t.mem_mb = bytes / 1024 / 1024;
    t
}

fn child(p: &Proc) -> Child {
    Child { pid: p.pid, name: p.name.clone(), mem_mb: p.mem / 1024 / 1024, cpu_pct: p.cpu }
}

/// Monta o monitor: `panes` = (id, nome, status, pid da raiz). O processo do Regente é `app`.
pub fn build(app: u32, panes: &[(String, String, String, u32)], procs: &[Proc]) -> Snapshot {
    let by_pid: HashMap<u32, &Proc> = procs.iter().map(|p| (p.pid, p)).collect();
    let app_desc = descendants(app, procs);
    let app_proc = by_pid.get(&app).copied();
    let app_tot = app_proc.map(|p| sum(std::iter::once(p))).unwrap_or_default();
    let ui: Vec<&Proc> = app_desc.iter().copied().filter(|p| p.name.eq_ignore_ascii_case(WEBVIEW)).collect();
    let mut seen_panes: HashSet<u32> = HashSet::new();
    let mut panes_out = Vec::new();
    for (id, name, status, pid) in panes {
        let Some(root) = by_pid.get(pid).copied() else { continue };
        // um processo só conta num painel (duas raízes com a mesma árvore não dobram a conta)
        let desc: Vec<&Proc> = descendants(*pid, procs).into_iter().filter(|p| seen_panes.insert(p.pid)).collect();
        seen_panes.insert(root.pid);
        let t = tree_totals(root, &desc);
        let mut children: Vec<Child> = desc.iter().map(|p| child(p)).collect();
        children.sort_by(|a, b| b.mem_mb.cmp(&a.mem_mb));
        panes_out.push(PaneRes {
            pane_id: id.clone(),
            name: name.clone(),
            status: status.clone(),
            mem_mb: t.mem_mb,
            cpu_pct: t.cpu_pct,
            procs: t.procs,
            children,
        });
    }
    let total = {
        let mut all: Vec<&Proc> = Vec::new();
        if let Some(p) = app_proc {
            all.push(p);
        }
        all.extend(app_desc.iter().copied());
        sum(all.into_iter())
    };
    Snapshot { total, panes: panes_out, ui: sum(ui.into_iter()), app: app_tot, cpu_ready: true }
}

/// Memória (MB) da árvore de um painel: é o que o ⋯ do painel mostra.
pub fn tree_mem_mb(root: u32, procs: &[Proc]) -> Option<u64> {
    let r = procs.iter().find(|p| p.pid == root)?;
    Some(tree_totals(r, &descendants(root, procs)).mem_mb)
}

struct Monitor {
    /// leituras feitas: a CPU só é real a partir da segunda (a primeira sai zerada)
    samples: u32,
    sys: sysinfo::System,
    at: Option<Instant>,
    procs: Vec<Proc>,
}

static MONITOR: Mutex<Option<Monitor>> = Mutex::new(None);

/// Lista de processos do sistema, relida no máximo a cada `SAMPLE_EVERY`.
pub fn procs() -> Vec<Proc> {
    let mut g = lk(&MONITOR);
    let m = g.get_or_insert_with(|| Monitor { samples: 0, sys: sysinfo::System::new(), at: None, procs: Vec::new() });
    let cores = std::thread::available_parallelism().map_or(1.0, |n| n.get() as f32);
    if m.at.is_none_or(|t| t.elapsed() >= SAMPLE_EVERY) {
        m.sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
        m.procs = m
            .sys
            .processes()
            .iter()
            .map(|(pid, p)| Proc {
                pid: pid.as_u32(),
                parent: p.parent().map(|x| x.as_u32()),
                start: p.start_time(),
                name: p.name().to_string_lossy().into_owned(),
                // memória privada (PrivateUsage no Windows), como o Gerenciador de Tarefas: o working set soma páginas
                // compartilhadas entre os processos da WebView e inflava o total
                mem: if cfg!(windows) { p.virtual_memory() } else { p.memory() },
                // sysinfo dá % de um núcleo; o Gerenciador de Tarefas divide pelos núcleos lógicos
                cpu: p.cpu_usage() / cores,
            })
            .collect();
        m.samples += 1;
        m.at = Some(Instant::now());
    }
    m.procs.clone()
}

/// A CPU já tem duas leituras (a primeira sai zerada e não entra no monitor).
pub fn cpu_ready() -> bool {
    lk(&MONITOR).as_ref().is_some_and(|m| m.samples >= 2)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(pid: u32, parent: Option<u32>, start: u64, name: &str, mb: u64) -> Proc {
        Proc { pid, parent, start, name: name.into(), mem: mb * 1024 * 1024, cpu: 1.0 }
    }

    fn pids(v: &[&Proc]) -> Vec<u32> {
        let mut x: Vec<u32> = v.iter().map(|p| p.pid).collect();
        x.sort();
        x
    }

    #[test]
    fn tree_sums_whole_pane_tree() {
        // cmd -> claude.cmd -> node -> servidor MCP (node); outro painel fora da árvore
        let procs = vec![
            p(10, Some(1), 100, "cmd.exe", 5),
            p(11, Some(10), 101, "claude.exe", 200),
            p(12, Some(11), 102, "node.exe", 180),
            p(13, Some(12), 103, "node.exe", 60),
            p(20, Some(1), 100, "cmd.exe", 7),
        ];
        let desc = descendants(10, &procs);
        assert_eq!(pids(&desc), [11, 12, 13]);
        assert_eq!(tree_mem_mb(10, &procs), Some(5 + 200 + 180 + 60));
        assert_eq!(tree_mem_mb(20, &procs), Some(7));
    }

    #[test]
    fn cycles_and_self_parent_terminate() {
        let procs = vec![p(10, Some(11), 100, "a", 1), p(11, Some(10), 100, "b", 1), p(12, Some(12), 100, "c", 1)];
        assert_eq!(pids(&descendants(10, &procs)), [11]);
        assert!(descendants(12, &procs).is_empty());
    }

    #[test]
    fn orphans_and_missing_root_are_ignored() {
        // pid 30 tem pai 99 que não existe: não é alcançado a partir de 10
        let procs = vec![p(10, None, 100, "cmd.exe", 1), p(30, Some(99), 100, "orfao.exe", 500)];
        assert!(descendants(10, &procs).is_empty());
        assert!(descendants(99, &procs).is_empty());
        assert_eq!(tree_mem_mb(99, &procs), None);
    }

    #[test]
    fn reused_pid_child_older_than_parent_is_skipped() {
        // o pid 10 foi reaproveitado (começou em 500); o processo 40 é antigo (100) e sobrou com pai 10
        let procs = vec![p(10, None, 500, "novo.exe", 1), p(40, Some(10), 100, "velho.exe", 300), p(41, Some(10), 600, "filho.exe", 2)];
        assert_eq!(pids(&descendants(10, &procs)), [41]);
    }

    #[test]
    fn snapshot_groups_webview_and_app() {
        let procs = vec![
            p(1, None, 10, "Regente.exe", 100),
            p(2, Some(1), 11, "msedgewebview2.exe", 300),
            p(3, Some(2), 12, "msedgewebview2.exe", 90),
            p(4, Some(1), 11, "cmd.exe", 4),
            p(5, Some(4), 12, "claude.exe", 150),
            p(6, Some(5), 13, "node.exe", 60),
            p(7, Some(1), 11, "msedgewebview2.exe", 0),
        ];
        let panes = vec![("p1".to_string(), "Claude A".to_string(), "ocioso".to_string(), 4u32), ("p2".to_string(), "Morto".to_string(), "parado".to_string(), 99u32)];
        let s = build(1, &panes, &procs);
        assert_eq!(s.app, Totals { mem_mb: 100, cpu_pct: 1.0, procs: 1 });
        assert_eq!(s.ui.procs, 3);
        assert_eq!(s.ui.mem_mb, 390);
        assert_eq!(s.panes.len(), 1, "painel sem processo some do monitor");
        assert_eq!(s.panes[0].mem_mb, 4 + 150 + 60);
        assert_eq!(s.panes[0].children.iter().map(|c| c.pid).collect::<Vec<_>>(), vec![5, 6]);
        assert_eq!(s.total.mem_mb, 100 + 390 + 4 + 150 + 60);
    }
}
