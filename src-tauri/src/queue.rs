use std::collections::{HashSet, VecDeque};
use std::time::{Duration, Instant};

/// Largest batch a mission takes, in panes: `create_mission` refuses more, and the adaptive ceiling never goes past it.
pub const MAX_BATCH: usize = 64;

/// Opens N items in order, at most `limit` at a time. The index is the slot in the grid.
pub struct SpawnQueue<T> {
    waiting: VecDeque<(usize, T)>,
    in_flight: HashSet<usize>,
    limit: usize,
}

impl<T> SpawnQueue<T> {
    pub fn new(items: Vec<T>, limit: usize) -> Self {
        Self { waiting: items.into_iter().enumerate().collect(), in_flight: HashSet::new(), limit: limit.max(1) }
    }

    pub fn limit(&self) -> usize {
        self.limit
    }

    /// Changes the concurrency for the next openings; the order of the waiting items is kept.
    pub fn set_limit(&mut self, limit: usize) {
        self.limit = limit.max(1);
    }

    pub fn next(&mut self) -> Option<(usize, T)> {
        if self.in_flight.len() >= self.limit {
            return None;
        }
        let (i, t) = self.waiting.pop_front()?;
        self.in_flight.insert(i);
        Some((i, t))
    }

    pub fn done(&mut self, idx: usize) {
        self.in_flight.remove(&idx);
    }

    /// Cancels an item still waiting. Returns false if it was already opening.
    pub fn cancel(&mut self, idx: usize) -> bool {
        let before = self.waiting.len();
        self.waiting.retain(|(i, _)| *i != idx);
        before != self.waiting.len()
    }

    pub fn is_finished(&self) -> bool {
        self.waiting.is_empty() && self.in_flight.is_empty()
    }
}

/// Concurrency for the next step, from the CPU load (percent) and the latency of the last spawns (ms): one step
/// down when the machine is busy or spawns are slow, one step up when both are calm. Always within 1 and
/// `ceiling` (the configured `spawn_concurrency`, at most MAX_BATCH).
pub fn adapt(current: usize, ceiling: usize, cpu_pct: f32, spawn_ms: u128) -> usize {
    let ceiling = ceiling.clamp(1, MAX_BATCH);
    let busy = cpu_pct >= 85.0 || spawn_ms >= 4000;
    let calm = cpu_pct < 60.0 && spawn_ms < 1500;
    if busy {
        current.saturating_sub(1).max(1)
    } else if calm {
        (current + 1).min(ceiling)
    } else {
        current.clamp(1, ceiling)
    }
}

/// Pura: um passo de ajuste por amostra fresca de CPU. Sem amostra nova neste tick, o limite fica como está.
pub fn adapt_on_sample(current: usize, ceiling: usize, cpu: Option<f32>, spawn_ms: u128) -> usize {
    match cpu {
        Some(c) => adapt(current, ceiling, c, spawn_ms),
        None => current,
    }
}

/// Reads the machine's load for `adapt`: CPU through sysinfo (sampled at most once a second, since it needs two
/// samples to measure) and the spawn latency as a smoothed average.
pub struct SpawnGauge {
    sys: sysinfo::System,
    sampled_at: Option<Instant>,
    latency: Option<u128>,
}

impl SpawnGauge {
    /// Já faz a primeira leitura da CPU: a próxima, um segundo depois, sai com base real.
    pub fn new() -> Self {
        let mut sys = sysinfo::System::new();
        sys.refresh_cpu_usage();
        Self { sys, sampled_at: None, latency: None }
    }

    /// `Some` só numa amostra nova (no máximo uma por segundo); `None` entre elas.
    pub fn cpu(&mut self) -> Option<f32> {
        if self.sampled_at.is_none_or(|t| t.elapsed() >= Duration::from_secs(1)) {
            self.sys.refresh_cpu_usage();
            self.sampled_at = Some(Instant::now());
            return Some(self.sys.global_cpu_usage());
        }
        None
    }

    pub fn record_spawn(&mut self, ms: u128) {
        self.latency = Some(match self.latency {
            None => ms,
            Some(prev) => (prev * 3 + ms) / 4,
        });
    }

    /// Zera a média de latência quando uma fila começa: os tempos de uma fila antiga não valem para a nova.
    pub fn reset_latency(&mut self) {
        self.latency = None;
    }

    pub fn latency(&self) -> u128 {
        self.latency.unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn respects_order_and_limit() {
        let mut q = SpawnQueue::new(vec!["a", "b", "c"], 2);
        assert_eq!(q.next(), Some((0, "a")));
        assert_eq!(q.next(), Some((1, "b")));
        assert_eq!(q.next(), None);
        q.done(0);
        assert_eq!(q.next(), Some((2, "c")));
        q.done(1);
        q.done(2);
        assert!(q.is_finished());
    }
    #[test]
    fn cancel_only_waiting() {
        let mut q = SpawnQueue::new(vec![1, 2, 3], 1);
        assert_eq!(q.next(), Some((0, 1)));
        assert!(!q.cancel(0));
        assert!(q.cancel(1));
        q.done(0);
        assert_eq!(q.next(), Some((2, 3)));
    }
    #[test]
    fn lowering_the_limit_keeps_the_order() {
        let mut q = SpawnQueue::new(vec!["a", "b", "c", "d"], 3);
        assert_eq!(q.next(), Some((0, "a")));
        assert_eq!(q.next(), Some((1, "b")));
        q.set_limit(1);
        assert_eq!(q.limit(), 1);
        assert_eq!(q.next(), None, "os que já abriram continuam contando");
        q.done(0);
        assert_eq!(q.next(), None, "um aberto ainda ocupa o único slot");
        q.done(1);
        assert_eq!(q.next(), Some((2, "c")));
        q.set_limit(0);
        assert_eq!(q.limit(), 1, "nunca abaixo de 1");
    }
    #[test]
    fn one_fresh_sample_moves_at_most_one_step() {
        assert_eq!(adapt_on_sample(5, 8, Some(10.0), 100), 6, "uma amostra calma: um passo");
        assert_eq!(adapt_on_sample(5, 8, Some(99.0), 100), 4, "uma amostra ocupada: um passo");
        assert_eq!(adapt_on_sample(5, 8, None, 100), 5, "sem amostra nova, nada muda");
        for cur in 1..=MAX_BATCH {
            for cpu in [0.0_f32, 70.0, 99.0] {
                let next = adapt_on_sample(cur, MAX_BATCH, Some(cpu), 100);
                assert!(next.abs_diff(cur) <= 1, "{cur} -> {next} com cpu {cpu}");
            }
        }
    }

    #[test]
    fn busy_or_slow_steps_down_and_never_below_one() {
        assert_eq!(adapt(4, 4, 90.0, 100), 3);
        assert_eq!(adapt(4, 4, 10.0, 5000), 3, "spawn lento também reduz");
        assert_eq!(adapt(1, 4, 99.0, 100), 1);
    }
    #[test]
    fn calm_steps_up_to_the_configured_ceiling() {
        assert_eq!(adapt(2, 4, 10.0, 200), 3);
        assert_eq!(adapt(4, 4, 10.0, 200), 4, "nunca passa do spawn_concurrency");
        // o teto é limitado a 64 mesmo que a configuração diga mais
        assert_eq!(adapt(64, 500, 10.0, 200), 64);
        assert_eq!(adapt(3, 0, 10.0, 200), 1, "teto zero vira 1");
    }
    #[test]
    fn middle_load_keeps_the_current_limit() {
        assert_eq!(adapt(3, 4, 70.0, 2000), 3);
        assert_eq!(adapt(9, 4, 70.0, 2000), 4, "fora do teto, volta para o teto");
    }
}
