use std::env;
use std::fmt;
use std::process::ExitCode;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Unit {
    // temperature
    Celsius,
    Fahrenheit,
    Kelvin,
    // length
    Meter,
    Foot,
    Mile,
    Kilometer,
    // weight
    Kilogram,
    Pound,
    Ounce,
}

use Unit::*;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    Temperature,
    Length,
    Weight,
}

impl Unit {
    fn kind(self) -> Kind {
        match self {
            Celsius | Fahrenheit | Kelvin => Kind::Temperature,
            Meter | Foot | Mile | Kilometer => Kind::Length,
            Kilogram | Pound | Ounce => Kind::Weight,
        }
    }

    fn symbol(self) -> &'static str {
        match self {
            Celsius => "°C",
            Fahrenheit => "°F",
            Kelvin => "K",
            Meter => "m",
            Foot => "ft",
            Mile => "mi",
            Kilometer => "km",
            Kilogram => "kg",
            Pound => "lb",
            Ounce => "oz",
        }
    }

    /// Factor to the base unit of its kind (meter / kilogram).
    /// Temperature is not linear, so it is handled separately.
    fn base_factor(self) -> f64 {
        match self {
            Meter => 1.0,
            Foot => 0.3048,
            Mile => 1609.344,
            Kilometer => 1000.0,
            Kilogram => 1.0,
            Pound => 0.453_592_37,
            Ounce => 0.028_349_523_125,
            Celsius | Fahrenheit | Kelvin => unreachable!("temperature has no linear factor"),
        }
    }
}

impl FromStr for Unit {
    type Err = ConvError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "c" | "celsius" => Ok(Celsius),
            "f" | "fahrenheit" => Ok(Fahrenheit),
            "k" | "kelvin" => Ok(Kelvin),
            "m" | "meter" | "meters" => Ok(Meter),
            "ft" | "foot" | "feet" => Ok(Foot),
            "mi" | "mile" | "miles" => Ok(Mile),
            "km" | "kilometer" | "kilometers" => Ok(Kilometer),
            "kg" | "kilogram" | "kilograms" => Ok(Kilogram),
            "lb" | "pound" | "pounds" => Ok(Pound),
            "oz" | "ounce" | "ounces" => Ok(Ounce),
            other => Err(ConvError::UnknownUnit(other.to_string())),
        }
    }
}

#[derive(Debug, PartialEq)]
enum ConvError {
    UnknownUnit(String),
    IncompatibleUnits(Unit, Unit),
    BadNumber(String),
    Usage,
}

impl fmt::Display for ConvError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConvError::UnknownUnit(s) => write!(f, "unknown unit '{s}'"),
            ConvError::IncompatibleUnits(a, b) => {
                write!(f, "can't convert {} to {}", a.symbol(), b.symbol())
            }
            ConvError::BadNumber(s) => write!(f, "'{s}' is not a number"),
            ConvError::Usage => write!(f, "usage: tempconv <value> <from> <to>   e.g. tempconv 100 c f"),
        }
    }
}

fn to_celsius(value: f64, from: Unit) -> f64 {
    match from {
        Celsius => value,
        Fahrenheit => (value - 32.0) * 5.0 / 9.0,
        Kelvin => value - 273.15,
        _ => unreachable!(),
    }
}

fn from_celsius(celsius: f64, to: Unit) -> f64 {
    match to {
        Celsius => celsius,
        Fahrenheit => celsius * 9.0 / 5.0 + 32.0,
        Kelvin => celsius + 273.15,
        _ => unreachable!(),
    }
}

fn convert(value: f64, from: Unit, to: Unit) -> Result<f64, ConvError> {
    if from.kind() != to.kind() {
        return Err(ConvError::IncompatibleUnits(from, to));
    }
    Ok(match from.kind() {
        Kind::Temperature => from_celsius(to_celsius(value, from), to),
        Kind::Length | Kind::Weight => value * from.base_factor() / to.base_factor(),
    })
}

fn run(args: &[String]) -> Result<String, ConvError> {
    let [value, from, to] = args else {
        return Err(ConvError::Usage);
    };
    let value: f64 = value
        .parse()
        .map_err(|_| ConvError::BadNumber(value.clone()))?;
    let from: Unit = from.parse()?;
    let to: Unit = to.parse()?;
    let result = convert(value, from, to)?;
    Ok(format!(
        "{value}{} = {result:.4}{}",
        from.symbol(),
        to.symbol()
    ))
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    match run(&args) {
        Ok(line) => {
            println!("{line}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("tempconv: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    #[test]
    fn temperature_conversions() {
        let cases = [
            (100.0, Celsius, Fahrenheit, 212.0),
            (32.0, Fahrenheit, Celsius, 0.0),
            (0.0, Celsius, Kelvin, 273.15),
            (300.0, Kelvin, Celsius, 26.85),
            (-40.0, Celsius, Fahrenheit, -40.0),
        ];
        for (value, from, to, want) in cases {
            let got = convert(value, from, to).unwrap();
            assert!(close(got, want), "{value} {from:?}->{to:?}: got {got}, want {want}");
        }
    }

    #[test]
    fn length_and_weight_conversions() {
        assert!(close(convert(1.0, Mile, Kilometer).unwrap(), 1.609344));
        assert!(close(convert(100.0, Foot, Meter).unwrap(), 30.48));
        assert!(close(convert(16.0, Ounce, Pound).unwrap(), 1.0));
        assert!(close(convert(1.0, Kilogram, Pound).unwrap(), 2.204_622_621_8));
    }

    #[test]
    fn incompatible_units_error() {
        assert_eq!(
            convert(1.0, Celsius, Meter),
            Err(ConvError::IncompatibleUnits(Celsius, Meter))
        );
    }

    #[test]
    fn unit_parsing() {
        assert_eq!("C".parse::<Unit>(), Ok(Celsius));
        assert_eq!("feet".parse::<Unit>(), Ok(Foot));
        assert!(matches!("furlong".parse::<Unit>(), Err(ConvError::UnknownUnit(_))));
    }

    #[test]
    fn run_formats_output() {
        let args: Vec<String> = ["100", "c", "f"].iter().map(|s| s.to_string()).collect();
        assert_eq!(run(&args).unwrap(), "100°C = 212.0000°F");
    }

    #[test]
    fn run_rejects_bad_input() {
        let args: Vec<String> = ["abc", "c", "f"].iter().map(|s| s.to_string()).collect();
        assert_eq!(run(&args), Err(ConvError::BadNumber("abc".into())));
        assert_eq!(run(&[]), Err(ConvError::Usage));
    }
}
