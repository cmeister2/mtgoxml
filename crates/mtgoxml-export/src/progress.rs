//! Terminal progress bars and plain stderr progress for unattended exports.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle};

use super::Result;

/// Minimum interval between ordinary progress lines in CI or redirected output.
const REPORT_INTERVAL: Duration = Duration::from_secs(5);

/// Shared rendering surface used by the bars and the logging bridge.
static MULTI_PROGRESS: OnceLock<MultiProgress> = OnceLock::new();

/// Initialize stderr logging with INFO defaults and the standard RUST_LOG filters.
///
/// # Errors
/// Returns an error if the process already registered a global logger.
pub(super) fn init_logging() -> Result<()> {
    let logger =
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
            .target(env_logger::Target::Stderr)
            .format_timestamp_secs()
            .build();
    let level = logger.filter();
    indicatif_log_bridge::LogWrapper::new(multi_progress().clone(), logger).try_init()?;
    log::set_max_level(level);
    Ok(())
}

/// Return the shared stderr display without creating a separate logging renderer.
fn multi_progress() -> &'static MultiProgress {
    MULTI_PROGRESS
        .get_or_init(|| MultiProgress::with_draw_target(ProgressDrawTarget::stderr_with_hz(5)))
}

/// One counted export stage whose output always goes to stderr.
pub(super) struct Progress {
    /// Short name shared by the bar and unattended log messages.
    label: &'static str,
    /// Library-owned rendering and completion counter.
    bar: ProgressBar,
    /// Whether the current stderr target needs plain newline output.
    plain: bool,
    /// Most recent operation, including one that stops before completion.
    detail: String,
    /// Time of the most recent unattended progress line.
    last_report: Instant,
    /// Successful completion suppresses the stopped message during destruction.
    finished: bool,
}

impl Progress {
    /// Start a counted stage on stderr, including a visible initial CI message.
    pub(super) fn new(label: &'static str, total: usize) -> Self {
        let style = ProgressStyle::with_template(
            "{prefix} [{wide_bar}] {pos}/{len} {elapsed_precise} {msg}",
        )
        .unwrap_or_else(|_| ProgressStyle::default_bar())
        .progress_chars("=> ");
        let bar = ProgressBar::with_draw_target(
            Some(total as u64),
            ProgressDrawTarget::stderr_with_hz(5),
        )
        .with_style(style)
        .with_prefix(label);
        let bar = if log::log_enabled!(log::Level::Info) {
            multi_progress().add(bar)
        } else {
            bar.set_draw_target(ProgressDrawTarget::hidden());
            bar
        };
        let progress = Self {
            label,
            plain: bar.is_hidden(),
            bar,
            detail: String::new(),
            last_report: Instant::now(),
            finished: false,
        };
        progress.report("started");
        progress
    }

    /// Show the next operation before it can block or fail.
    pub(super) fn set_message(&mut self, detail: &str) {
        self.detail.clear();
        self.detail.push_str(detail);
        self.bar.set_message(self.detail.clone());
        self.report_if_due();
    }

    /// Count one completed operation and periodically report unattended progress.
    pub(super) fn advance(&mut self) {
        self.bar.inc(1);
        self.report_if_due();
    }

    /// End a successful stage with its completed count and elapsed time.
    pub(super) fn finish(mut self) {
        self.finished = true;
        self.bar.finish_and_clear();
        multi_progress().remove(&self.bar);
        self.report("complete");
    }

    /// Limit CI log volume while retaining regular updates during active work.
    fn report_if_due(&mut self) {
        if self.plain && self.last_report.elapsed() >= REPORT_INTERVAL {
            self.report("in progress");
            self.last_report = Instant::now();
        }
    }

    /// Send stage counts through the same logger as other exporter messages.
    fn report(&self, status: &str) {
        log::info!(
            "{}: {}/{}, {} ({:.1}s){}{}",
            self.label,
            self.bar.position(),
            self.bar.length().unwrap_or(0),
            status,
            self.bar.elapsed().as_secs_f64(),
            if self.detail.is_empty() { "" } else { ": " },
            self.detail,
        );
    }
}

impl Drop for Progress {
    fn drop(&mut self) {
        if !self.finished {
            // Remove abandoned bars before logging their incomplete count.
            self.bar.abandon_with_message("stopped");
            multi_progress().remove(&self.bar);
            log::warn!(
                "{}: {}/{}, stopped ({:.1}s){}{}",
                self.label,
                self.bar.position(),
                self.bar.length().unwrap_or(0),
                self.bar.elapsed().as_secs_f64(),
                if self.detail.is_empty() { "" } else { ": " },
                self.detail,
            );
        }
    }
}
