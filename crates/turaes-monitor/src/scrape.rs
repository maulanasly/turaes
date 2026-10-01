//! A minimal Prometheus text-format parser.
//!
//! Scoped to what turaes reads from tonggeret apps: HTTP series, business
//! counters, and visitor series. Not a general-purpose exposition parser — but
//! it handles labels, escaping and `# HELP`/`# TYPE` directives.

use std::collections::BTreeMap;

/// One parsed sample.
#[derive(Debug, Clone, PartialEq)]
pub struct Sample {
    /// Metric name.
    pub name: String,
    /// Label set.
    pub labels: BTreeMap<String, String>,
    /// Sample value.
    pub value: f64,
}

impl Sample {
    /// Look up a label value.
    pub fn label(&self, key: &str) -> Option<&str> {
        self.labels.get(key).map(String::as_str)
    }
}

/// Parse a Prometheus text exposition body into samples.
///
/// Malformed lines are skipped rather than failing the whole scrape, so one bad
/// line never blanks a dashboard.
pub fn parse(body: &str) -> Vec<Sample> {
    let mut samples = Vec::new();
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(sample) = parse_line(line) {
            samples.push(sample);
        }
    }
    samples
}

fn parse_line(line: &str) -> Option<Sample> {
    // Split name+labels from the value: the value begins after the closing
    // brace (if any) at the first whitespace.
    let (head, tail) = if let Some(open) = line.find('{') {
        let close = line[open..].find('}').map(|i| open + i)?;
        let head = &line[..=close];
        let tail = line[close + 1..].trim_start();
        (head, tail)
    } else {
        let mut it = line.splitn(2, char::is_whitespace);
        let head = it.next()?;
        let tail = it.next().unwrap_or("");
        (head, tail)
    };

    let (name, labels) = if let Some(open) = head.find('{') {
        let name = head[..open].to_string();
        let close = head.rfind('}')?;
        let labels = parse_labels(&head[open + 1..close]);
        (name, labels)
    } else {
        (head.to_string(), BTreeMap::new())
    };

    let value_str = tail.split_whitespace().next()?;
    let value: f64 = match value_str {
        "+Inf" => f64::INFINITY,
        "-Inf" => f64::NEG_INFINITY,
        "NaN" => f64::NAN,
        _ => value_str.parse().ok()?,
    };

    if name.is_empty() {
        return None;
    }
    Some(Sample {
        name,
        labels,
        value,
    })
}

fn parse_labels(input: &str) -> BTreeMap<String, String> {
    let mut labels = BTreeMap::new();
    let mut chars = input.chars().peekable();
    while chars.peek().is_some() {
        // key
        let mut key = String::new();
        while let Some(&c) = chars.peek() {
            if c == '=' {
                break;
            }
            key.push(c);
            chars.next();
        }
        if chars.peek() != Some(&'=') {
            break;
        }
        chars.next(); // consume '='
        if chars.peek() != Some(&'"') {
            // malformed; skip to next comma
            for c in chars.by_ref() {
                if c == ',' {
                    break;
                }
            }
            continue;
        }
        chars.next(); // consume opening quote
        let mut val = String::new();
        while let Some(c) = chars.next() {
            match c {
                '\\' => {
                    if let Some(&next) = chars.peek() {
                        chars.next();
                        val.push(match next {
                            'n' => '\n',
                            't' => '\t',
                            other => other,
                        });
                    }
                }
                '"' => break,
                other => val.push(other),
            }
        }
        labels.insert(key.trim().to_string(), val);
        // consume up to and including the comma separator
        for c in chars.by_ref() {
            if c == ',' {
                break;
            }
        }
    }
    labels
}

/// Aggregate of the tonggeret visitor series for one region.
#[derive(Debug, Clone, PartialEq)]
pub struct VisitSample {
    /// Region label (`unknown` when no CDN header).
    pub region: String,
    /// `visitors_total` (counter).
    pub visits: i64,
    /// `unique_visitors_estimate` (gauge, latest value).
    pub uniques: i64,
}

/// Fold `visitors_total` + `unique_visitors_estimate` samples into per-region
/// rows. Uniques are never summed — the gauge's latest value wins.
pub fn visitor_samples(samples: &[Sample]) -> Vec<VisitSample> {
    let mut by_region: BTreeMap<String, VisitSample> = BTreeMap::new();
    for s in samples {
        let region = s.label("region").unwrap_or("unknown").to_string();
        let entry = by_region.entry(region.clone()).or_insert(VisitSample {
            region,
            visits: 0,
            uniques: 0,
        });
        match s.name.as_str() {
            "visitors_total" => entry.visits = s.value as i64,
            "unique_visitors_estimate" => entry.uniques = s.value as i64,
            _ => {}
        }
    }
    by_region.into_values().collect()
}

/// Sum a counter metric across all label sets.
pub fn sum(samples: &[Sample], name: &str) -> f64 {
    samples
        .iter()
        .filter(|s| s.name == name)
        .map(|s| s.value)
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXPOSITION: &str = r#"# HELP http_requests_total Total requests.
# TYPE http_requests_total counter
http_requests_total{method="GET",route="/health"} 12
http_requests_total{method="POST",route="/api"} 3
# TYPE visitors_total counter
visitors_total{region="unknown"} 42
# TYPE unique_visitors_estimate gauge
unique_visitors_estimate{region="unknown"} 7
custom_metric 3.5
"#;

    #[test]
    fn parses_samples_and_skips_directives() {
        let samples = parse(EXPOSITION);
        assert_eq!(samples.len(), 5);
        assert_eq!(samples[0].name, "http_requests_total");
        assert_eq!(samples[0].label("route"), Some("/health"));
        assert_eq!(samples[0].value, 12.0);
    }

    #[test]
    fn sums_counters() {
        let samples = parse(EXPOSITION);
        assert_eq!(sum(&samples, "http_requests_total"), 15.0);
    }

    #[test]
    fn extracts_visitors() {
        let samples = parse(EXPOSITION);
        let visits = visitor_samples(&samples);
        assert_eq!(visits.len(), 1);
        assert_eq!(visits[0].region, "unknown");
        assert_eq!(visits[0].visits, 42);
        assert_eq!(visits[0].uniques, 7);
    }

    #[test]
    fn handles_multiple_regions_and_latest_uniques() {
        let body = "visitors_total{region=\"ID\"} 10\nvisitors_total{region=\"SG\"} 4\nunique_visitors_estimate{region=\"ID\"} 6\nunique_visitors_estimate{region=\"ID\"} 9\n";
        let visits = visitor_samples(&parse(body));
        assert_eq!(visits.len(), 2);
        let id = visits.iter().find(|v| v.region == "ID").unwrap();
        assert_eq!(id.visits, 10);
        assert_eq!(id.uniques, 9);
    }

    #[test]
    fn malformed_lines_are_skipped() {
        let samples = parse("good 1\nthis is not metrics\n{\n");
        assert_eq!(
            samples,
            vec![Sample {
                name: "good".into(),
                labels: BTreeMap::new(),
                value: 1.0
            }]
        );
    }
}
