//! shapes — one trait, many types, two kinds of dispatch.
//!
//!   shapes circle:2 rect:3x4 tri:3,4,5
//!
//! Prints a report sorted by area. The same trait is used through trait
//! objects (`Box<dyn Shape>`, heterogeneous list) and through generics
//! (`fn describe<T: Shape>` — monomorphized, no vtable).

use std::env;
use std::f64::consts::PI;
use std::process::ExitCode;

trait Shape {
    fn name(&self) -> &'static str;
    fn area(&self) -> f64;
    fn perimeter(&self) -> f64;

    /// Default method: every shape gets it for free.
    fn describe(&self) -> String {
        format!(
            "{:<10} area {:>8.2}  perimeter {:>8.2}",
            self.name(),
            self.area(),
            self.perimeter()
        )
    }
}

struct Circle {
    radius: f64,
}

struct Rect {
    width: f64,
    height: f64,
}

struct Triangle {
    a: f64,
    b: f64,
    c: f64,
}

impl Shape for Circle {
    fn name(&self) -> &'static str {
        "circle"
    }
    fn area(&self) -> f64 {
        PI * self.radius * self.radius
    }
    fn perimeter(&self) -> f64 {
        2.0 * PI * self.radius
    }
}

impl Shape for Rect {
    fn name(&self) -> &'static str {
        "rect"
    }
    fn area(&self) -> f64 {
        self.width * self.height
    }
    fn perimeter(&self) -> f64 {
        2.0 * (self.width + self.height)
    }
}

impl Triangle {
    fn is_valid(&self) -> bool {
        let Triangle { a, b, c } = *self;
        a > 0.0 && b > 0.0 && c > 0.0 && a + b > c && b + c > a && a + c > b
    }
}

impl Shape for Triangle {
    fn name(&self) -> &'static str {
        "triangle"
    }
    fn area(&self) -> f64 {
        // Heron's formula.
        let s = self.perimeter() / 2.0;
        (s * (s - self.a) * (s - self.b) * (s - self.c)).sqrt()
    }
    fn perimeter(&self) -> f64 {
        self.a + self.b + self.c
    }
}

/// Static dispatch: compiled separately for every concrete T.
/// Only the tests call it — it exists to contrast with `Box<dyn Shape>`.
#[cfg_attr(not(test), allow(dead_code))]
fn describe_generic<T: Shape>(shape: &T) -> String {
    shape.describe()
}

/// Parses "circle:2", "rect:3x4", "tri:3,4,5" into a boxed trait object.
fn parse_shape(spec: &str) -> Result<Box<dyn Shape>, String> {
    let (kind, dims) = spec
        .split_once(':')
        .ok_or_else(|| format!("bad spec '{spec}', expected kind:dims"))?;

    let nums = |sep: char| -> Result<Vec<f64>, String> {
        dims.split(sep)
            .map(|p| p.parse::<f64>().map_err(|_| format!("bad number in '{spec}'")))
            .collect()
    };

    match kind {
        "circle" => {
            let radius: f64 = dims.parse().map_err(|_| format!("bad radius in '{spec}'"))?;
            if radius <= 0.0 {
                return Err(format!("radius must be positive in '{spec}'"));
            }
            Ok(Box::new(Circle { radius }))
        }
        "rect" => match nums('x')?.as_slice() {
            &[width, height] if width > 0.0 && height > 0.0 => Ok(Box::new(Rect { width, height })),
            _ => Err(format!("rect needs WxH with positive sides in '{spec}'")),
        },
        "tri" => match nums(',')?.as_slice() {
            &[a, b, c] => {
                let t = Triangle { a, b, c };
                if t.is_valid() {
                    Ok(Box::new(t))
                } else {
                    Err(format!("sides {a},{b},{c} don't form a triangle"))
                }
            }
            _ => Err(format!("tri needs three sides in '{spec}'")),
        },
        other => Err(format!("unknown shape '{other}'")),
    }
}

fn report(shapes: &mut Vec<Box<dyn Shape>>) -> String {
    shapes.sort_by(|a, b| a.area().total_cmp(&b.area()));
    let total: f64 = shapes.iter().map(|s| s.area()).sum();
    let mut lines: Vec<String> = shapes.iter().map(|s| s.describe()).collect();
    lines.push(format!("total area {total:.2}"));
    lines.join("\n")
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: shapes <kind:dims>...   e.g. shapes circle:2 rect:3x4 tri:3,4,5");
        return ExitCode::FAILURE;
    }

    // Heterogeneous collection: this is what trait objects are for.
    let mut shapes: Vec<Box<dyn Shape>> = Vec::new();
    for spec in &args {
        match parse_shape(spec) {
            Ok(shape) => shapes.push(shape),
            Err(e) => {
                eprintln!("shapes: {e}");
                return ExitCode::FAILURE;
            }
        }
    }
    println!("{}", report(&mut shapes));
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn circle_math() {
        let c = Circle { radius: 1.0 };
        assert!(close(c.area(), PI));
        assert!(close(c.perimeter(), 2.0 * PI));
    }

    #[test]
    fn rect_math() {
        let r = Rect { width: 3.0, height: 4.0 };
        assert!(close(r.area(), 12.0));
        assert!(close(r.perimeter(), 14.0));
    }

    #[test]
    fn triangle_heron() {
        let t = Triangle { a: 3.0, b: 4.0, c: 5.0 };
        assert!(close(t.area(), 6.0));
        assert!(close(t.perimeter(), 12.0));
    }

    #[test]
    fn invalid_triangle_rejected() {
        assert!(parse_shape("tri:1,1,10").is_err());
    }

    #[test]
    fn both_dispatch_styles_agree() {
        let c = Circle { radius: 2.0 };
        let via_generic = describe_generic(&c);
        let boxed: Box<dyn Shape> = Box::new(Circle { radius: 2.0 });
        assert_eq!(via_generic, boxed.describe());
    }

    #[test]
    fn parsing_specs() {
        assert_eq!(parse_shape("circle:2").unwrap().name(), "circle");
        assert_eq!(parse_shape("rect:3x4").unwrap().name(), "rect");
        assert_eq!(parse_shape("tri:3,4,5").unwrap().name(), "triangle");
        assert!(parse_shape("blob:9").is_err());
        assert!(parse_shape("rect:3").is_err());
        assert!(parse_shape("circle:-1").is_err());
    }

    #[test]
    fn report_sorts_by_area() {
        let mut shapes: Vec<Box<dyn Shape>> = vec![
            Box::new(Rect { width: 10.0, height: 10.0 }),
            Box::new(Circle { radius: 1.0 }),
        ];
        let out = report(&mut shapes);
        let circle_pos = out.find("circle").unwrap();
        let rect_pos = out.find("rect").unwrap();
        assert!(circle_pos < rect_pos, "smaller area first:\n{out}");
        assert!(out.ends_with("total area 103.14"));
    }
}
