//! tictactoe — two players on one keyboard, cells addressed 1-9.

use std::fmt;
use std::io::{self, Write};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Player {
    X,
    O,
}

impl Player {
    fn other(self) -> Player {
        match self {
            Player::X => Player::O,
            Player::O => Player::X,
        }
    }
}

impl fmt::Display for Player {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Player::X => "X",
            Player::O => "O",
        })
    }
}

#[derive(Debug, PartialEq)]
enum GameState {
    InProgress,
    Won(Player),
    Draw,
}

/// `None` = empty cell. Indexed [row][col].
#[derive(Debug, Default, Clone, PartialEq)]
struct Board([[Option<Player>; 3]; 3]);

#[derive(Debug, PartialEq)]
enum MoveError {
    BadCell,
    Occupied,
}

impl Board {
    /// Places a mark in cell 1..=9 (numbered like a phone keypad).
    fn play(&mut self, cell: u8, player: Player) -> Result<(), MoveError> {
        if !(1..=9).contains(&cell) {
            return Err(MoveError::BadCell);
        }
        let (row, col) = (((cell - 1) / 3) as usize, ((cell - 1) % 3) as usize);
        match self.0[row][col] {
            Some(_) => Err(MoveError::Occupied),
            None => {
                self.0[row][col] = Some(player);
                Ok(())
            }
        }
    }

    fn state(&self) -> GameState {
        const LINES: [[(usize, usize); 3]; 8] = [
            [(0, 0), (0, 1), (0, 2)],
            [(1, 0), (1, 1), (1, 2)],
            [(2, 0), (2, 1), (2, 2)],
            [(0, 0), (1, 0), (2, 0)],
            [(0, 1), (1, 1), (2, 1)],
            [(0, 2), (1, 2), (2, 2)],
            [(0, 0), (1, 1), (2, 2)],
            [(0, 2), (1, 1), (2, 0)],
        ];
        for line in LINES {
            let [a, b, c] = line.map(|(r, col)| self.0[r][col]);
            if let Some(player) = a
                && a == b
                && b == c
            {
                return GameState::Won(player);
            }
        }
        let full = self.0.iter().flatten().all(Option::is_some);
        if full { GameState::Draw } else { GameState::InProgress }
    }
}

impl fmt::Display for Board {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (r, row) in self.0.iter().enumerate() {
            let cells: Vec<String> = row
                .iter()
                .enumerate()
                .map(|(c, cell)| match cell {
                    Some(p) => format!(" {p} "),
                    None => format!(" {} ", r * 3 + c + 1),
                })
                .collect();
            writeln!(f, "{}", cells.join("|"))?;
            if r < 2 {
                writeln!(f, "---+---+---")?;
            }
        }
        Ok(())
    }
}

fn read_line(prompt: &str) -> String {
    print!("{prompt}");
    io::stdout().flush().expect("flush stdout");
    let mut line = String::new();
    io::stdin().read_line(&mut line).expect("read stdin");
    line.trim().to_string()
}

fn main() {
    loop {
        let mut board = Board::default();
        let mut player = Player::X;
        println!("{board}");

        loop {
            let input = read_line(&format!("{player} > "));
            let Ok(cell) = input.parse::<u8>() else {
                println!("Enter a cell number 1-9.");
                continue;
            };
            match board.play(cell, player) {
                Err(MoveError::BadCell) => {
                    println!("Cells are 1-9.");
                    continue;
                }
                Err(MoveError::Occupied) => {
                    println!("That cell is taken.");
                    continue;
                }
                Ok(()) => {}
            }
            println!("{board}");
            match board.state() {
                GameState::InProgress => player = player.other(),
                GameState::Won(winner) => {
                    println!("{winner} wins!");
                    break;
                }
                GameState::Draw => {
                    println!("Draw.");
                    break;
                }
            }
        }

        if !read_line("Play again? [y/N] ").eq_ignore_ascii_case("y") {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Plays cells alternately starting with X.
    fn play_all(cells: &[u8]) -> Board {
        let mut board = Board::default();
        let mut player = Player::X;
        for &cell in cells {
            board.play(cell, player).unwrap();
            player = player.other();
        }
        board
    }

    #[test]
    fn row_column_and_diagonal_wins() {
        // X: 1 2 3 (top row)
        assert_eq!(play_all(&[1, 4, 2, 5, 3]).state(), GameState::Won(Player::X));
        // O: 4 5 6 (middle row)
        assert_eq!(play_all(&[1, 4, 2, 5, 9, 6]).state(), GameState::Won(Player::O));
        // X: 1 4 7 (left column)
        assert_eq!(play_all(&[1, 2, 4, 5, 7]).state(), GameState::Won(Player::X));
        // X: 1 5 9 (diagonal)
        assert_eq!(play_all(&[1, 2, 5, 3, 9]).state(), GameState::Won(Player::X));
        // X: 3 5 7 (anti-diagonal)
        assert_eq!(play_all(&[3, 2, 5, 4, 7]).state(), GameState::Won(Player::X));
    }

    #[test]
    fn draw_detection() {
        // X O X / X O O / O X X — full board, nobody wins.
        let board = play_all(&[1, 2, 3, 5, 4, 6, 8, 7, 9]);
        assert_eq!(board.state(), GameState::Draw);
    }

    #[test]
    fn game_in_progress() {
        assert_eq!(play_all(&[1, 2]).state(), GameState::InProgress);
        assert_eq!(Board::default().state(), GameState::InProgress);
    }

    #[test]
    fn invalid_moves_are_rejected() {
        let mut board = Board::default();
        assert_eq!(board.play(0, Player::X), Err(MoveError::BadCell));
        assert_eq!(board.play(10, Player::X), Err(MoveError::BadCell));
        board.play(5, Player::X).unwrap();
        assert_eq!(board.play(5, Player::O), Err(MoveError::Occupied));
    }
}
