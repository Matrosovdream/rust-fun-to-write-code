//! lifegame — Conway's Game of Life in the terminal. The capstone: nothing
//! new, everything used — structs, iterators, file I/O, timing, rendering.
//!
//!   lifegame                        # random soup
//!   lifegame glider.cells           # load a pattern (.cells format)
//!   lifegame --width 60 --height 25 --density 0.25
//!
//! Controls: space pause/resume · n step · +/- speed · r randomize · q quit

use std::collections::hash_map::DefaultHasher;
use std::fmt;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::{self, Write};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::Parser;
use crossterm::event::{self, Event, KeyCode};
use crossterm::{cursor, execute, terminal};
use rand::Rng;

/// Conway's Game of Life
#[derive(Debug, Parser)]
#[command(version, about)]
struct Args {
    /// Pattern file in .cells format ('.' dead, 'O'/'#'/'*' alive)
    file: Option<String>,

    /// Board width
    #[arg(long, default_value_t = 60)]
    width: usize,

    /// Board height
    #[arg(long, default_value_t = 25)]
    height: usize,

    /// Fill density for random boards (0.0..1.0)
    #[arg(long, default_value_t = 0.3)]
    density: f64,

    /// Milliseconds between generations
    #[arg(long, default_value_t = 100)]
    delay: u64,
}

#[derive(Debug, Clone, PartialEq, Hash)]
struct Grid {
    width: usize,
    height: usize,
    cells: Vec<bool>, // row-major; edges wrap (toroidal)
}

impl Grid {
    fn empty(width: usize, height: usize) -> Grid {
        Grid { width, height, cells: vec![false; width * height] }
    }

    fn random(width: usize, height: usize, density: f64, rng: &mut impl Rng) -> Grid {
        let mut grid = Grid::empty(width, height);
        for cell in &mut grid.cells {
            *cell = rng.random_bool(density.clamp(0.0, 1.0));
        }
        grid
    }

    fn get(&self, x: usize, y: usize) -> bool {
        self.cells[y * self.width + x]
    }

    fn set(&mut self, x: usize, y: usize, alive: bool) {
        self.cells[y * self.width + x] = alive;
    }

    /// Counts the 8 neighbours, wrapping around the edges.
    fn neighbours(&self, x: usize, y: usize) -> u8 {
        let mut count = 0;
        for dy in [self.height - 1, 0, 1] {
            for dx in [self.width - 1, 0, 1] {
                if dx == 0 && dy == 0 {
                    continue;
                }
                let nx = (x + dx) % self.width;
                let ny = (y + dy) % self.height;
                if self.get(nx, ny) {
                    count += 1;
                }
            }
        }
        count
    }

    /// One generation, double-buffered: read `self`, write `next`.
    fn step(&self) -> Grid {
        let mut next = Grid::empty(self.width, self.height);
        for y in 0..self.height {
            for x in 0..self.width {
                let alive = self.get(x, y);
                let n = self.neighbours(x, y);
                // The whole game: survive on 2-3, get born on 3.
                next.set(x, y, matches!((alive, n), (true, 2) | (true, 3) | (false, 3)));
            }
        }
        next
    }

    fn population(&self) -> usize {
        self.cells.iter().filter(|&&c| c).count()
    }

    fn fingerprint(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.cells.hash(&mut hasher);
        hasher.finish()
    }

    /// Parses .cells text centered onto a width×height board.
    fn from_pattern(text: &str, width: usize, height: usize) -> Result<Grid> {
        let rows: Vec<&str> = text
            .lines()
            .filter(|l| !l.starts_with('!') && !l.trim().is_empty())
            .collect();
        if rows.is_empty() {
            bail!("pattern file has no cells");
        }
        let pattern_h = rows.len();
        let pattern_w = rows.iter().map(|r| r.chars().count()).max().unwrap_or(0);
        if pattern_w > width || pattern_h > height {
            bail!("pattern is {pattern_w}x{pattern_h}, board is only {width}x{height}");
        }

        let mut grid = Grid::empty(width, height);
        let (off_x, off_y) = ((width - pattern_w) / 2, (height - pattern_h) / 2);
        for (y, row) in rows.iter().enumerate() {
            for (x, c) in row.chars().enumerate() {
                match c {
                    'O' | '#' | '*' => grid.set(off_x + x, off_y + y, true),
                    '.' | ' ' => {}
                    other => bail!("unexpected character '{other}' in pattern"),
                }
            }
        }
        Ok(grid)
    }
}

impl fmt::Display for Grid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for y in 0..self.height {
            for x in 0..self.width {
                f.write_str(if self.get(x, y) { "█" } else { " " })?;
            }
            f.write_str("\r\n")?;
        }
        Ok(())
    }
}

/// Remembers recent fingerprints to detect still lifes and oscillators.
struct StabilityDetector {
    recent: Vec<u64>,
}

impl StabilityDetector {
    const WINDOW: usize = 12;

    fn new() -> Self {
        StabilityDetector { recent: Vec::new() }
    }

    /// Feeds one generation; returns the period once the board repeats.
    fn check(&mut self, fingerprint: u64) -> Option<usize> {
        let period = self
            .recent
            .iter()
            .rev()
            .position(|&f| f == fingerprint)
            .map(|p| p + 1);
        self.recent.push(fingerprint);
        if self.recent.len() > Self::WINDOW {
            self.recent.remove(0);
        }
        period
    }
}

struct Ui;

/// RAII guard: raw mode and the alternate screen are always restored,
/// even on panic or early return.
impl Ui {
    fn enter() -> Result<Ui> {
        terminal::enable_raw_mode()?;
        execute!(io::stdout(), terminal::EnterAlternateScreen, cursor::Hide)?;
        Ok(Ui)
    }
}

impl Drop for Ui {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), cursor::Show, terminal::LeaveAlternateScreen);
        let _ = terminal::disable_raw_mode();
    }
}

enum Action {
    None,
    Quit,
    TogglePause,
    SingleStep,
    Faster,
    Slower,
    Randomize,
}

fn read_action(timeout: Duration) -> Result<Action> {
    if !event::poll(timeout)? {
        return Ok(Action::None);
    }
    let Event::Key(key) = event::read()? else {
        return Ok(Action::None);
    };
    Ok(match key.code {
        KeyCode::Char('q') | KeyCode::Esc => Action::Quit,
        KeyCode::Char(' ') => Action::TogglePause,
        KeyCode::Char('n') => Action::SingleStep,
        KeyCode::Char('+') | KeyCode::Char('=') => Action::Faster,
        KeyCode::Char('-') => Action::Slower,
        KeyCode::Char('r') => Action::Randomize,
        _ => Action::None,
    })
}

fn main() -> Result<()> {
    let args = Args::parse();
    let mut rng = rand::rng();

    let mut grid = match &args.file {
        Some(path) => {
            let text = fs::read_to_string(path).with_context(|| format!("failed to read {path}"))?;
            Grid::from_pattern(&text, args.width, args.height)?
        }
        None => Grid::random(args.width, args.height, args.density, &mut rng),
    };

    let _ui = Ui::enter()?;
    let mut out = io::stdout();

    let mut delay = Duration::from_millis(args.delay.max(10));
    let mut running = true;
    let mut generation: u64 = 0;
    let mut detector = StabilityDetector::new();
    let mut stable: Option<usize> = None;
    let mut last_step = Instant::now();

    loop {
        execute!(out, cursor::MoveTo(0, 0))?;
        write!(out, "{grid}")?;
        let status = match (running, stable) {
            (false, _) => "paused (space resumes, n steps)".to_string(),
            (true, Some(1)) => "stable — still life".to_string(),
            (true, Some(period)) => format!("stable — oscillator, period {period}"),
            (true, None) => format!("running at {}ms/gen", delay.as_millis()),
        };
        write!(
            out,
            "gen {generation} | pop {} | {status} | q quits          \r\n",
            grid.population()
        )?;
        out.flush()?;

        match read_action(Duration::from_millis(15))? {
            Action::Quit => break,
            Action::TogglePause => running = !running,
            Action::SingleStep if !running => {
                grid = grid.step();
                generation += 1;
                stable = detector.check(grid.fingerprint());
            }
            Action::SingleStep => {}
            Action::Faster => delay = (delay / 2).max(Duration::from_millis(10)),
            Action::Slower => delay = (delay * 2).min(Duration::from_secs(2)),
            Action::Randomize => {
                grid = Grid::random(args.width, args.height, args.density, &mut rng);
                generation = 0;
                detector = StabilityDetector::new();
                stable = None;
            }
            Action::None => {}
        }

        if running && last_step.elapsed() >= delay {
            grid = grid.step();
            generation += 1;
            stable = detector.check(grid.fingerprint());
            last_step = Instant::now();
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a grid from rows of '.'/'O' at exact size (no centering).
    fn grid_of(rows: &[&str]) -> Grid {
        let mut grid = Grid::empty(rows[0].len(), rows.len());
        for (y, row) in rows.iter().enumerate() {
            for (x, c) in row.chars().enumerate() {
                grid.set(x, y, c == 'O');
            }
        }
        grid
    }

    #[test]
    fn block_is_a_still_life() {
        let block = grid_of(&[
            "....",
            ".OO.",
            ".OO.",
            "....",
        ]);
        assert_eq!(block.step(), block);
    }

    #[test]
    fn blinker_oscillates_with_period_2() {
        let horizontal = grid_of(&[
            ".....",
            ".....",
            ".OOO.",
            ".....",
            ".....",
        ]);
        let vertical = grid_of(&[
            ".....",
            "..O..",
            "..O..",
            "..O..",
            ".....",
        ]);
        assert_eq!(horizontal.step(), vertical);
        assert_eq!(vertical.step(), horizontal);
    }

    #[test]
    fn lonely_cells_die_and_empty_stays_empty() {
        let lonely = grid_of(&["O..", "...", "..O"]);
        assert_eq!(lonely.step().population(), 0);
        let empty = Grid::empty(5, 5);
        assert_eq!(empty.step(), empty);
    }

    #[test]
    fn glider_returns_shifted_after_4_generations() {
        // On a big enough board, a glider repeats its shape one cell
        // down-right every 4 generations.
        let mut grid = Grid::empty(10, 10);
        for (x, y) in [(1, 0), (2, 1), (0, 2), (1, 2), (2, 2)] {
            grid.set(x, y, true);
        }
        let mut stepped = grid.clone();
        for _ in 0..4 {
            stepped = stepped.step();
        }
        let mut shifted = Grid::empty(10, 10);
        for (x, y) in [(2, 1), (3, 2), (1, 3), (2, 3), (3, 3)] {
            shifted.set(x, y, true);
        }
        assert_eq!(stepped, shifted);
    }

    #[test]
    fn edges_wrap_around() {
        // A blinker crossing the edge still oscillates.
        let mut grid = Grid::empty(5, 5);
        for x in [4, 0, 1] {
            grid.set(x, 2, true);
        }
        let twice = grid.step().step();
        assert_eq!(twice, grid);
    }

    #[test]
    fn stability_detection() {
        let mut detector = StabilityDetector::new();
        assert_eq!(detector.check(1), None);
        assert_eq!(detector.check(1), Some(1)); // still life
        let mut detector = StabilityDetector::new();
        assert_eq!(detector.check(1), None);
        assert_eq!(detector.check(2), None);
        assert_eq!(detector.check(1), Some(2)); // period-2 oscillator
    }

    #[test]
    fn pattern_parsing_centers_and_validates() {
        let grid = Grid::from_pattern("!comment\n.O.\nO.O\n", 7, 5).unwrap();
        assert_eq!(grid.population(), 3);
        assert!(grid.get(3, 1)); // centered: offset (2, 1)
        assert!(grid.get(2, 2) && grid.get(4, 2));

        assert!(Grid::from_pattern("", 5, 5).is_err());
        assert!(Grid::from_pattern("OOOOOOOOOO", 5, 5).is_err());
        assert!(Grid::from_pattern("O?O", 5, 5).is_err());
    }
}
