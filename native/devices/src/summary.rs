use std::fmt;
use std::time::Duration;

use engine::block::RATE;

use crate::report::Report;

/// The callback timing lines measure-sink and measure-clock both print, so a timer's numbers
/// read beside a device's in docs/decisions.
pub struct Summary<'a> {
    pub report: &'a Report,
    pub period_frames: u32,
    /// How long the stream ran.
    pub ran: Duration,
}

impl fmt::Display for Summary<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self { report, period_frames, ran } = *self;
        let period = Duration::from_secs_f64(f64::from(period_frames) / f64::from(RATE));
        let late = period.mul_f64(1.5);
        let optional = |value: Option<Duration>| value.map(ms).unwrap_or_else(|| "-".into());
        let interval = |fraction: f64| optional(report.percentile(fraction));
        let yes = |flag: bool| if flag { "yes" } else { "no" };

        writeln!(f, "realtime   {}", yes(report.realtime))?;
        writeln!(
            f,
            "callbacks  {} in {:.1} s, {:.0} expected",
            report.callbacks,
            ran.as_secs_f64(),
            ran.as_secs_f64() / period.as_secs_f64(),
        )?;
        writeln!(
            f,
            "interval   min {}  p50 {}  p90 {}  p99 {}  p99.9 {}  max {} ms",
            optional(report.shortest),
            interval(0.5),
            interval(0.9),
            interval(0.99),
            interval(0.999),
            optional(report.longest),
        )?;
        writeln!(f, "late       {} over 1.5 periods ({} ms)", report.at_least(late), ms(late))?;
        writeln!(f, "underruns  {}", report.underruns)?;
        writeln!(f, "lost       {}", yes(report.lost))?;
        write!(f, "failed     {}", yes(report.failed))
    }
}

pub fn ms(duration: Duration) -> String {
    format!("{:.2}", duration.as_secs_f64() * 1000.0)
}
