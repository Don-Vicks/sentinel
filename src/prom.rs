//! Prometheus text exposition (`GET /metrics`), so Sentinel can be scraped by
//! Prometheus or Grafana Agent and graphed or alerted on next to everything else.

use std::collections::HashSet;
use std::fmt::Write;

/// Builds the exposition format, declaring each metric's HELP and TYPE once.
#[derive(Default)]
pub struct Exposition {
    out: String,
    declared: HashSet<String>,
}

/// Label values escape backslash, quote and newline.
pub fn escape(v: &str) -> String {
    v.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n")
}

impl Exposition {
    fn sample(&mut self, kind: &str, name: &str, help: &str, labels: &[(&str, &str)], value: f64) {
        // NaN and infinity are valid in the format but break dashboards; a missing sample is clearer.
        if !value.is_finite() {
            return;
        }
        if self.declared.insert(name.to_string()) {
            let _ = writeln!(self.out, "# HELP {name} {help}\n# TYPE {name} {kind}");
        }
        let labels = if labels.is_empty() {
            String::new()
        } else {
            format!("{{{}}}", labels.iter().map(|(k, v)| format!("{k}=\"{}\"", escape(v))).collect::<Vec<_>>().join(","))
        };
        let _ = writeln!(self.out, "{name}{labels} {value}");
    }

    pub fn gauge(&mut self, name: &str, help: &str, labels: &[(&str, &str)], value: f64) {
        self.sample("gauge", name, help, labels, value);
    }

    pub fn counter(&mut self, name: &str, help: &str, labels: &[(&str, &str)], value: f64) {
        self.sample("counter", name, help, labels, value);
    }

    pub fn finish(self) -> String {
        self.out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declares_each_metric_once_and_escapes_labels() {
        let mut e = Exposition::default();
        e.gauge("sentinel_program_tps", "Transactions per second.", &[("program", "Pump.fun")], 12.5);
        e.gauge("sentinel_program_tps", "Transactions per second.", &[("program", "A \"quoted\" \\ name\nnext")], 3.0);
        e.counter("sentinel_dropped_total", "Dropped.", &[], 7.0);
        e.gauge("sentinel_bad", "Never shown.", &[], f64::NAN);
        let text = e.finish();
        assert_eq!(text.matches("# TYPE sentinel_program_tps gauge").count(), 1);
        assert!(text.contains("sentinel_program_tps{program=\"Pump.fun\"} 12.5\n"), "{text}");
        assert!(text.contains(r#"program="A \"quoted\" \\ name\nnext""#), "{text}");
        assert!(text.contains("# TYPE sentinel_dropped_total counter\nsentinel_dropped_total 7\n"), "{text}");
        assert!(!text.contains("sentinel_bad"), "a NaN is left out");
        assert!(text.lines().all(|l| !l.is_empty()), "no blank lines");
    }
}
