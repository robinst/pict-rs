use std::time::Instant;

pub(super) struct PushMetricsGuard {
    queue: &'static str,
    armed: bool,
}

pub(super) struct PopMetricsGuard {
    queue: &'static str,
    start: Instant,
    armed: bool,
}

pub(super) struct WaitMetricsGuard {
    start: Instant,
    armed: bool,
}

impl PushMetricsGuard {
    pub(super) fn guard(queue: &'static str) -> Self {
        Self { queue, armed: true }
    }

    pub(super) fn disarm(mut self) {
        self.armed = false;
    }
}

impl PopMetricsGuard {
    pub(super) fn guard(queue: &'static str) -> Self {
        Self {
            queue,
            start: Instant::now(),
            armed: true,
        }
    }

    pub(super) fn disarm(mut self) {
        self.armed = false;
    }
}

impl WaitMetricsGuard {
    pub(super) fn guard() -> Self {
        Self {
            start: Instant::now(),
            armed: true,
        }
    }

    pub(super) fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for PushMetricsGuard {
    fn drop(&mut self) {
        metrics::increment_counter!("pict-rs.queue.push", "completed" => (!self.armed).to_string(), "queue" => self.queue);
    }
}

impl Drop for PopMetricsGuard {
    fn drop(&mut self) {
        metrics::histogram!("pict-rs.queue.pop.duration", self.start.elapsed().as_secs_f64(), "completed" => (!self.armed).to_string(), "queue" => self.queue);
        metrics::increment_counter!("pict-rs.queue.pop", "completed" => (!self.armed).to_string(), "queue" => self.queue);
    }
}

impl Drop for WaitMetricsGuard {
    fn drop(&mut self) {
        metrics::histogram!("pict-rs.upload.wait.duration", self.start.elapsed().as_secs_f64(), "completed" => (!self.armed).to_string());
        metrics::increment_counter!("pict-rs.upload.wait", "completed" => (!self.armed).to_string());
    }
}
