//! pomodoro — work/break cycles with a live countdown.
//!
//!   pomodoro                      # 25/5 × 4
//!   pomodoro -w 50 -b 10 -c 2
//!
//! Ctrl-C pauses and asks whether to resume; the countdown redraws itself
//! in place with '\r'. The schedule is a pure state machine — all the
//! wall-clock and signal handling stays in `main`, which is why the tests
//! never sleep.

use std::io::{self, BufRead, Write};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::Duration;

use clap::Parser;

/// Pomodoro timer
#[derive(Debug, Parser)]
#[command(version, about)]
struct Args {
    /// Work minutes
    #[arg(short, long, default_value_t = 25)]
    work: u64,

    /// Break minutes
    #[arg(short, long, default_value_t = 5)]
    brk: u64,

    /// Number of work cycles
    #[arg(short, long, default_value_t = 4)]
    cycles: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    Work,
    Break,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Phase {
    kind: Kind,
    number: u64,
    seconds: u64,
}

/// work, break, work, break, ..., work — no trailing break.
fn build_phases(work_min: u64, break_min: u64, cycles: u64) -> Vec<Phase> {
    let mut phases = Vec::new();
    for n in 1..=cycles {
        phases.push(Phase { kind: Kind::Work, number: n, seconds: work_min * 60 });
        if n < cycles {
            phases.push(Phase { kind: Kind::Break, number: n, seconds: break_min * 60 });
        }
    }
    phases
}

fn format_mmss(seconds: u64) -> String {
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}

fn label(phase: &Phase, total_cycles: u64) -> String {
    match phase.kind {
        Kind::Work => format!("work {}/{}", phase.number, total_cycles),
        Kind::Break => format!("break {}/{}", phase.number, total_cycles - 1),
    }
}

/// Stats accumulated across the session.
#[derive(Debug, Default, PartialEq)]
struct Summary {
    work_seconds: u64,
    work_phases: u64,
    breaks: u64,
}

impl Summary {
    fn record(&mut self, phase: &Phase, seconds_done: u64) {
        match phase.kind {
            Kind::Work => {
                self.work_seconds += seconds_done;
                if seconds_done == phase.seconds {
                    self.work_phases += 1;
                }
            }
            Kind::Break => self.breaks += 1,
        }
    }

    fn render(&self) -> String {
        format!(
            "focused {} across {} full pomodoro(s), {} break(s)",
            format_mmss(self.work_seconds),
            self.work_phases,
            self.breaks
        )
    }
}

enum TickResult {
    Finished,
    Quit { seconds_done: u64 },
}

/// Counts one phase down, polling the Ctrl-C channel between sleeps.
fn run_phase(phase: &Phase, name: &str, interrupts: &Receiver<()>) -> TickResult {
    let mut remaining = phase.seconds;
    while remaining > 0 {
        print!("\r{name}  {}   (ctrl-c to pause) ", format_mmss(remaining));
        io::stdout().flush().expect("flush stdout");

        match interrupts.try_recv() {
            Ok(()) => {
                println!("\npaused at {}", format_mmss(remaining));
                if !ask_resume() {
                    return TickResult::Quit { seconds_done: phase.seconds - remaining };
                }
            }
            Err(TryRecvError::Empty) => {
                thread::sleep(Duration::from_secs(1));
                remaining -= 1;
            }
            Err(TryRecvError::Disconnected) => unreachable!("sender lives in the handler"),
        }
    }
    println!("\r{name}  done!                          ");
    TickResult::Finished
}

fn ask_resume() -> bool {
    print!("resume? [Y/n] ");
    io::stdout().flush().expect("flush stdout");
    let mut line = String::new();
    io::stdin().lock().read_line(&mut line).expect("read stdin");
    !line.trim().eq_ignore_ascii_case("n")
}

fn main() {
    let args = Args::parse();
    let phases = build_phases(args.work, args.brk, args.cycles);

    // The handler runs on its own thread: a channel carries the signal
    // back to the timer loop. First mpsc of the series.
    let (tx, rx) = mpsc::channel();
    ctrlc::set_handler(move || {
        let _ = tx.send(());
    })
    .expect("install ctrl-c handler");

    println!(
        "pomodoro: {}min work / {}min break × {}",
        args.work, args.brk, args.cycles
    );

    let mut summary = Summary::default();
    for phase in &phases {
        match run_phase(phase, &label(phase, args.cycles), &rx) {
            TickResult::Finished => summary.record(phase, phase.seconds),
            TickResult::Quit { seconds_done } => {
                summary.record(phase, seconds_done);
                println!("stopped early. {}", summary.render());
                return;
            }
        }
    }
    println!("session complete! {}", summary.render());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phases_alternate_without_trailing_break() {
        let phases = build_phases(25, 5, 3);
        let kinds: Vec<Kind> = phases.iter().map(|p| p.kind).collect();
        assert_eq!(
            kinds,
            vec![Kind::Work, Kind::Break, Kind::Work, Kind::Break, Kind::Work]
        );
        assert_eq!(phases[0].seconds, 25 * 60);
        assert_eq!(phases[1].seconds, 5 * 60);
    }

    #[test]
    fn single_cycle_has_no_break() {
        let phases = build_phases(25, 5, 1);
        assert_eq!(phases.len(), 1);
        assert_eq!(phases[0].kind, Kind::Work);
    }

    #[test]
    fn mmss_formatting() {
        assert_eq!(format_mmss(0), "00:00");
        assert_eq!(format_mmss(61), "01:01");
        assert_eq!(format_mmss(25 * 60), "25:00");
        assert_eq!(format_mmss(3599), "59:59");
    }

    #[test]
    fn labels() {
        let phases = build_phases(25, 5, 2);
        assert_eq!(label(&phases[0], 2), "work 1/2");
        assert_eq!(label(&phases[1], 2), "break 1/1");
    }

    #[test]
    fn summary_counts_only_full_work_phases() {
        let phases = build_phases(25, 5, 2);
        let mut summary = Summary::default();
        summary.record(&phases[0], phases[0].seconds); // full work
        summary.record(&phases[1], phases[1].seconds); // break
        summary.record(&phases[2], 100); // interrupted work
        assert_eq!(
            summary,
            Summary { work_seconds: 25 * 60 + 100, work_phases: 1, breaks: 1 }
        );
        assert!(summary.render().contains("1 full pomodoro"));
    }
}
