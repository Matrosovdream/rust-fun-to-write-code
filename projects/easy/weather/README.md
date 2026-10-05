# weather — HTTP API client

Current weather for any city via Open-Meteo (free, no API key): geocode the
name, fetch the forecast, print a report. `--json` dumps the raw response.

```sh
cargo run -- Berlin
cargo run -- "New York" --json
cargo test        # runs offline against canned JSON
```

## Covers

`reqwest::blocking` with query params, deserializing real-world JSON into
minimal structs (extra fields ignored, `Option` for absent ones), separating
"city not found" from network errors, `anyhow::Context` on every fallible
step, offline tests with canned responses.

## Rewrite exercises

1. Rewrite from scratch; start by saving the two real API responses and
   writing the structs + tests against them.
2. Add a 3-day forecast table (`daily=temperature_2m_max,...` params).
3. Cache geocoding results in a JSON file so repeated cities skip a request.
4. Handle ambiguity: `--all` lists the top 5 matching places to choose from.
5. Port it to async (`tokio` + non-blocking reqwest) and fetch geocoding
   for several cities concurrently with `join_all` — your bridge to the
   medium tier.
