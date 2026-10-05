//! weather — current weather for a city, via the Open-Meteo API (no key).
//!
//!   weather Berlin
//!   weather "New York" --json
//!
//! Two requests: geocode the city name, then fetch the forecast for the
//! coordinates. The response structs mirror only the fields we need —
//! serde ignores the rest.

use anyhow::{Context, Result, bail};
use clap::Parser;
use serde::Deserialize;

/// Current weather for a city
#[derive(Debug, Parser)]
#[command(version, about)]
struct Args {
    /// City name, e.g. Berlin or "New York"
    city: Vec<String>,

    /// Print the raw API response instead of a report
    #[arg(long)]
    json: bool,
}

// ---- geocoding ----------------------------------------------------------

#[derive(Debug, Deserialize)]
struct GeoResponse {
    /// Absent (not empty!) when nothing matches — hence Option.
    results: Option<Vec<Place>>,
}

#[derive(Debug, Deserialize, Clone, PartialEq)]
struct Place {
    name: String,
    latitude: f64,
    longitude: f64,
    #[serde(default)]
    country: Option<String>,
    #[serde(default)]
    admin1: Option<String>, // state / region
}

impl Place {
    fn describe(&self) -> String {
        let mut parts = vec![self.name.clone()];
        parts.extend(self.admin1.clone());
        parts.extend(self.country.clone());
        parts.join(", ")
    }
}

// ---- forecast -----------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ForecastResponse {
    current: Current,
}

#[derive(Debug, Deserialize, PartialEq)]
struct Current {
    temperature_2m: f64,
    apparent_temperature: f64,
    relative_humidity_2m: f64,
    wind_speed_10m: f64,
    weather_code: u16,
}

/// WMO weather interpretation codes, the interesting subset.
fn describe_code(code: u16) -> &'static str {
    match code {
        0 => "clear sky",
        1 | 2 => "partly cloudy",
        3 => "overcast",
        45 | 48 => "fog",
        51..=57 => "drizzle",
        61..=67 => "rain",
        71..=77 => "snow",
        80..=82 => "rain showers",
        85 | 86 => "snow showers",
        95 => "thunderstorm",
        96 | 99 => "thunderstorm with hail",
        _ => "something unusual",
    }
}

fn report(place: &Place, current: &Current) -> String {
    format!(
        "{}\n  {}, {:.1}°C (feels like {:.1}°C)\n  humidity {:.0}%, wind {:.1} km/h",
        place.describe(),
        describe_code(current.weather_code),
        current.temperature_2m,
        current.apparent_temperature,
        current.relative_humidity_2m,
        current.wind_speed_10m,
    )
}

// ---- api calls ----------------------------------------------------------

const GEO_URL: &str = "https://geocoding-api.open-meteo.com/v1/search";
const FORECAST_URL: &str = "https://api.open-meteo.com/v1/forecast";

fn geocode(client: &reqwest::blocking::Client, city: &str) -> Result<Place> {
    let response: GeoResponse = client
        .get(GEO_URL)
        .query(&[("name", city), ("count", "1")])
        .send()
        .context("geocoding request failed — network down?")?
        .error_for_status()
        .context("geocoding API returned an error")?
        .json()
        .context("geocoding response was not the JSON we expected")?;

    // "Not found" is not a network error — it gets its own message.
    match response.results.unwrap_or_default().into_iter().next() {
        Some(place) => Ok(place),
        None => bail!("city '{city}' not found — try a bigger town nearby"),
    }
}

fn fetch_forecast_raw(client: &reqwest::blocking::Client, place: &Place) -> Result<String> {
    let text = client
        .get(FORECAST_URL)
        .query(&[
            ("latitude", place.latitude.to_string()),
            ("longitude", place.longitude.to_string()),
            (
                "current",
                "temperature_2m,apparent_temperature,relative_humidity_2m,wind_speed_10m,weather_code"
                    .to_string(),
            ),
        ])
        .send()
        .context("forecast request failed — network down?")?
        .error_for_status()
        .context("forecast API returned an error")?
        .text()?;
    Ok(text)
}

fn main() -> Result<()> {
    let args = Args::parse();
    let city = args.city.join(" ");
    if city.trim().is_empty() {
        bail!("usage: weather <city>");
    }

    let client = reqwest::blocking::Client::builder()
        .user_agent("weather-cli-learning-project")
        .build()?;

    let place = geocode(&client, &city)?;
    let raw = fetch_forecast_raw(&client, &place)?;

    if args.json {
        println!("{raw}");
        return Ok(());
    }

    let forecast: ForecastResponse =
        serde_json::from_str(&raw).context("forecast JSON didn't match our structs")?;
    println!("{}", report(&place, &forecast.current));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Canned responses captured from the real API — tests never go online.
    const GEO_JSON: &str = r#"{
        "results": [{
            "id": 2950159, "name": "Berlin",
            "latitude": 52.52437, "longitude": 13.41053,
            "country": "Germany", "admin1": "Land Berlin", "population": 3426354
        }],
        "generationtime_ms": 0.7
    }"#;

    const FORECAST_JSON: &str = r#"{
        "latitude": 52.52, "longitude": 13.42,
        "current_units": {"temperature_2m": "°C"},
        "current": {
            "time": "2026-10-01T12:00", "interval": 900,
            "temperature_2m": 14.3, "apparent_temperature": 12.1,
            "relative_humidity_2m": 71, "wind_speed_10m": 18.4,
            "weather_code": 61
        }
    }"#;

    #[test]
    fn deserializes_geocoding_response() {
        let geo: GeoResponse = serde_json::from_str(GEO_JSON).unwrap();
        let place = geo.results.unwrap().into_iter().next().unwrap();
        assert_eq!(place.name, "Berlin");
        assert_eq!(place.country.as_deref(), Some("Germany"));
        assert!((place.latitude - 52.52437).abs() < 1e-9);
        assert_eq!(place.describe(), "Berlin, Land Berlin, Germany");
    }

    #[test]
    fn empty_geocoding_results_deserialize_as_none() {
        let geo: GeoResponse = serde_json::from_str(r#"{"generationtime_ms": 0.3}"#).unwrap();
        assert!(geo.results.is_none());
    }

    #[test]
    fn deserializes_forecast_response() {
        let forecast: ForecastResponse = serde_json::from_str(FORECAST_JSON).unwrap();
        assert_eq!(
            forecast.current,
            Current {
                temperature_2m: 14.3,
                apparent_temperature: 12.1,
                relative_humidity_2m: 71.0,
                wind_speed_10m: 18.4,
                weather_code: 61,
            }
        );
    }

    #[test]
    fn weather_codes_have_descriptions() {
        assert_eq!(describe_code(0), "clear sky");
        assert_eq!(describe_code(61), "rain");
        assert_eq!(describe_code(95), "thunderstorm");
        assert_eq!(describe_code(254), "something unusual");
    }

    #[test]
    fn report_formats_the_essentials() {
        let geo: GeoResponse = serde_json::from_str(GEO_JSON).unwrap();
        let place = geo.results.unwrap().into_iter().next().unwrap();
        let forecast: ForecastResponse = serde_json::from_str(FORECAST_JSON).unwrap();
        let text = report(&place, &forecast.current);
        assert!(text.contains("Berlin"));
        assert!(text.contains("rain, 14.3°C (feels like 12.1°C)"));
        assert!(text.contains("humidity 71%, wind 18.4 km/h"));
    }
}
