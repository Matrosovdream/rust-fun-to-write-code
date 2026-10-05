//! orgtree — builds a tree from an indented text file and renders it.
//!
//!   orgtree team.txt                # whole tree
//!   orgtree team.txt "VP Sales"     # just that subtree
//!
//! Input: one name per line, two spaces of indent per level, one root.
//!
//! Why `Box` matters here: `struct Node { children: Vec<Node> }` compiles
//! because Vec stores its elements on the heap already. A *direct*
//! self-reference like `next: Option<Node>` would make the type infinitely
//! sized — that's when you must write `Option<Box<Node>>`. This project uses
//! `Box<Node>` for children to make the indirection visible and the
//! ownership explicit: each child is owned, boxed, and dropped with its
//! parent.

use std::env;
use std::fmt;
use std::fs;
use std::io::{self, Read};
use std::process::ExitCode;

#[derive(Debug, PartialEq)]
struct Node {
    name: String,
    // clippy rightly notes Vec<Node> would do — the Box stays to make the
    // indirection explicit; see the module comment.
    #[allow(clippy::vec_box)]
    children: Vec<Box<Node>>,
}

impl Node {
    fn new(name: &str) -> Node {
        Node { name: name.to_string(), children: Vec::new() }
    }

    /// Everyone in this subtree, including self.
    fn headcount(&self) -> usize {
        1 + self.children.iter().map(|c| c.headcount()).sum::<usize>()
    }

    /// Levels below self (a leaf has depth 0).
    fn depth(&self) -> usize {
        self.children.iter().map(|c| c.depth() + 1).max().unwrap_or(0)
    }

    fn find(&self, name: &str) -> Option<&Node> {
        if self.name == name {
            return Some(self);
        }
        self.children.iter().find_map(|c| c.find(name))
    }

    /// Descends `depth` levels along the last children, then attaches.
    fn insert_at(&mut self, depth: usize, name: &str) -> Result<(), ParseError> {
        if depth == 0 {
            self.children.push(Box::new(Node::new(name)));
            return Ok(());
        }
        match self.children.last_mut() {
            Some(last) => last.insert_at(depth - 1, name),
            None => Err(ParseError::BadIndent(name.to_string())),
        }
    }
}

#[derive(Debug, PartialEq)]
enum ParseError {
    Empty,
    BadIndent(String),
    MultipleRoots(String),
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::Empty => write!(f, "input is empty"),
            ParseError::BadIndent(name) => write!(f, "'{name}' is indented too deep"),
            ParseError::MultipleRoots(name) => {
                write!(f, "'{name}' is a second root — indent it under the first")
            }
        }
    }
}

fn indent_of(line: &str) -> usize {
    (line.len() - line.trim_start_matches(' ').len()) / 2
}

fn parse(text: &str) -> Result<Node, ParseError> {
    let mut root: Option<Node> = None;
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let depth = indent_of(line);
        let name = line.trim();
        match (&mut root, depth) {
            (None, 0) => root = Some(Node::new(name)),
            (None, _) => return Err(ParseError::BadIndent(name.to_string())),
            (Some(_), 0) => return Err(ParseError::MultipleRoots(name.to_string())),
            (Some(root), depth) => root.insert_at(depth - 1, name)?,
        }
    }
    root.ok_or(ParseError::Empty)
}

/// Recursive rendering with box-drawing characters. `prefix` accumulates
/// the pipes of all ancestor levels.
fn render_into(node: &Node, prefix: &str, out: &mut String) {
    for (i, child) in node.children.iter().enumerate() {
        let last = i == node.children.len() - 1;
        let (branch, extension) = if last { ("└── ", "    ") } else { ("├── ", "│   ") };
        out.push_str(prefix);
        out.push_str(branch);
        out.push_str(&child.name);
        out.push('\n');
        render_into(child, &format!("{prefix}{extension}"), out);
    }
}

fn render(root: &Node) -> String {
    let mut out = format!("{}\n", root.name);
    render_into(root, "", &mut out);
    out
}

fn main() -> ExitCode {
    let input = match env::args().nth(1) {
        Some(path) => fs::read_to_string(&path).map_err(|e| format!("{path}: {e}")),
        None => {
            let mut buf = String::new();
            io::stdin()
                .read_to_string(&mut buf)
                .map(|_| buf)
                .map_err(|e| e.to_string())
        }
    };
    let input = match input {
        Ok(text) => text,
        Err(e) => {
            eprintln!("orgtree: {e}");
            return ExitCode::FAILURE;
        }
    };

    let root = match parse(&input) {
        Ok(root) => root,
        Err(e) => {
            eprintln!("orgtree: {e}");
            return ExitCode::FAILURE;
        }
    };

    // With a second argument, show only that subtree.
    let node = match env::args().nth(2) {
        None => &root,
        Some(name) => match root.find(&name) {
            Some(node) => node,
            None => {
                eprintln!("orgtree: nobody named '{name}' in the tree");
                return ExitCode::FAILURE;
            }
        },
    };

    print!("{}", render(node));
    println!("\nheadcount: {}, depth: {}", node.headcount(), node.depth());
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEAM: &str = "\
CEO
  VP Eng
    Dev A
    Dev B
  VP Sales
    Rep";

    #[test]
    fn parses_structure() {
        let root = parse(TEAM).unwrap();
        assert_eq!(root.name, "CEO");
        assert_eq!(root.children.len(), 2);
        assert_eq!(root.children[0].children.len(), 2);
        assert_eq!(root.find("Dev B").unwrap().name, "Dev B");
        assert!(root.find("Ghost").is_none());
    }

    #[test]
    fn headcount_and_depth() {
        let root = parse(TEAM).unwrap();
        assert_eq!(root.headcount(), 6);
        assert_eq!(root.depth(), 2);
        assert_eq!(root.find("Rep").unwrap().depth(), 0);
        assert_eq!(root.find("VP Eng").unwrap().headcount(), 3);
    }

    #[test]
    fn renders_with_box_chars() {
        let root = parse(TEAM).unwrap();
        let expected = "\
CEO
├── VP Eng
│   ├── Dev A
│   └── Dev B
└── VP Sales
    └── Rep
";
        assert_eq!(render(&root), expected);
    }

    #[test]
    fn parse_errors() {
        assert_eq!(parse(""), Err(ParseError::Empty));
        assert_eq!(parse("  A"), Err(ParseError::BadIndent("A".into())));
        assert_eq!(parse("A\nB"), Err(ParseError::MultipleRoots("B".into())));
        assert_eq!(parse("A\n    B"), Err(ParseError::BadIndent("B".into())));
    }

    #[test]
    fn single_node_tree() {
        let root = parse("Solo\n").unwrap();
        assert_eq!(root.headcount(), 1);
        assert_eq!(root.depth(), 0);
        assert_eq!(render(&root), "Solo\n");
    }
}
