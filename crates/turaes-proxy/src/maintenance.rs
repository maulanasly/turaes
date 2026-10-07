//! Parked-host maintenance page: zero-build HTML served with 503 when a known
//! hostname has no live upstream (stopped app, unhealthy slots, maintenance).
//! Pure functions, so the page is unit-tested without the Pingora feature.

use crate::router::ParkedReason;

/// Headline for the page. Names the application slug, never the hostname.
pub fn headline(reason: ParkedReason) -> &'static str {
    match reason {
        ParkedReason::Stopped => "temporarily unavailable",
        ParkedReason::Unhealthy => "experiencing problems",
    }
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

/// Full HTML body for a parked host.
pub fn body(app: &str, reason: ParkedReason) -> String {
    format!(
        "<!DOCTYPE html>\n\
         <html lang=\"en\">\n\
         <head><meta charset=\"utf-8\" />\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\" />\
         <title>{app} {headline} — turaes</title>\
         <style>:root{{color-scheme:dark}}body{{margin:0;background:#0f1115;color:#e6e8ee;\
         font:16px/1.5 ui-sans-serif,system-ui,sans-serif;display:flex;min-height:100vh;\
         align-items:center;justify-content:center}}main{{text-align:center;padding:24px}}\
         h1{{font-size:28px;margin:0 0 8px}}.app{{color:#17c3b2}}\
         p{{color:#8b93a7;margin:0 0 4px}}footer{{margin-top:16px;font-size:12px}}</style></head>\n\
         <body><main><p>turaes</p><h1><span class=\"app\">{app}</span> {headline}</h1>\
         <p>Please try again in a minute.</p><footer>HTTP 503</footer></main></body>\n\
         </html>\n",
        app = escape(app),
        headline = headline(reason),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headlines_name_the_app_not_the_host() {
        let html = body("beruang-dev", ParkedReason::Stopped);
        assert!(html.contains("beruang-dev"));
        assert!(html.contains("temporarily unavailable"));
        assert!(!html.contains("beruang-dev.rayakala.ink"));
        let html = body("dev-lp", ParkedReason::Unhealthy);
        assert!(html.contains("experiencing problems"));
    }

    #[test]
    fn app_names_are_escaped() {
        let html = body("<script>alert(1)</script>", ParkedReason::Stopped);
        assert!(!html.contains("<script>"));
        assert!(html.contains("&lt;script&gt;"));
    }
}
