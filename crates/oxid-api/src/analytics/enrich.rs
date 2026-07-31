//! Turning request headers into the dimensions the dashboard groups by.
//!
//! Every function here is pure and takes what it needs, so the whole module is
//! testable without a request, a server or a runtime — which matters because the
//! user-agent rules are the kind of thing that is only ever verified by feeding it
//! strings that really occurred.
//!
//! **Why hand-rolled rather than a parser crate.** User-agent strings have no
//! grammar and lie on purpose: every browser still claims `Mozilla/5.0` for
//! compatibility with a negotiation that ended in the nineties, Chrome claims
//! `Safari`, and Edge claims both. A full parser resolves that with a database of
//! hundreds of regexes — real precision, at the cost of a dependency on the
//! redirect's path and a boot that compiles the lot. What the dashboard needs is a
//! handful of buckets, and buckets are what substring rules are good at. The
//! trade is deliberate: this errs on exotic agents, and the table test below is
//! where that gets corrected as real traffic shows up.

use axum::http::HeaderMap;
use url::Url;

/// What one request says about who made it. Borrowed rather than owned: these are
/// a fixed vocabulary, and the caller allocates only when building the event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Agent {
    pub device: &'static str,
    pub os: &'static str,
    pub browser: &'static str,
    /// `1` for a bot, `0` otherwise — ClickHouse has no boolean.
    pub is_bot: u8,
}

/// The bucket everything unrecognised falls into. A named constant because it is
/// also what the dashboard shows, and "other" growing is the signal that the rules
/// below have fallen behind real traffic.
const OTHER: &str = "other";

/// Substrings that mean "not a person".
///
/// The second group is what makes this worth doing at all for a shortener: a link
/// pasted into a chat is fetched by the platform to build a preview, before any
/// human clicks it. Counting those as visitors would inflate every number on the
/// dashboard, and the busiest links would be the ones shared in the most groups
/// rather than the ones people actually opened.
const BOT_MARKERS: [&str; 21] = [
    // Generic crawlers.
    "bot",
    "crawler",
    "spider",
    "slurp",
    "crawling",
    "archiver",
    "monitor",
    "uptime",
    // Tools, which are honest about themselves.
    "curl",
    "wget",
    "python-requests",
    "go-http-client",
    "okhttp",
    "headless",
    // Link unfurlers. These do not say "bot".
    "facebookexternalhit",
    "whatsapp",
    "telegrambot",
    "discordbot",
    "slackbot",
    "twitterbot",
    "linkedinbot",
];

/// Reads a header as a string, empty when absent or not valid UTF-8.
fn header<'a>(headers: &'a HeaderMap, name: &str) -> &'a str {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
}

/// Two-letter country from Cloudflare.
///
/// Set by the edge on every request, so it needs no IP database here — and it is
/// the only reason country is cheap. `XX` is Cloudflare's own value for unknown
/// and `T1` for Tor; both pass through as-is, because "we could not tell" is
/// information the dashboard should show rather than hide.
///
/// Bounded to four characters. The header is client-supplied on any path that
/// does not come through Cloudflare, and an unbounded value would land in a
/// `LowCardinality` column that assumes a small vocabulary.
pub fn country(headers: &HeaderMap) -> String {
    let raw = header(headers, "cf-ipcountry");

    if raw.is_empty() || raw.len() > 4 || !raw.chars().all(|c| c.is_ascii_alphanumeric()) {
        return String::new();
    }

    raw.to_ascii_uppercase()
}

/// Primary language subtag, lowercased.
///
/// `pt-BR,pt;q=0.9,en;q=0.8` becomes `pt`. The region is dropped on purpose: the
/// column is `LowCardinality`, and a dashboard listing `pt` above `pt-BR` and
/// `pt-PT` separately answers "which languages" better than one listing all three.
pub fn lang(headers: &HeaderMap) -> String {
    let raw = header(headers, "accept-language");

    let primary = raw
        .split(',')
        .next()
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .split('-')
        .next()
        .unwrap_or("")
        .trim();

    if primary.is_empty() || primary.len() > 8 || !primary.chars().all(|c| c.is_ascii_alphabetic())
    {
        return String::new();
    }

    primary.to_ascii_lowercase()
}

/// Host of the referring page, without the scheme or path.
///
/// The host alone, never the full URL: a referring path can carry a search query,
/// a session token or an internal document name, and none of that is the
/// dashboard's business. Storing the host answers "where do these people come
/// from" while keeping the analytics table free of someone else's private URLs.
///
/// Empty for most clicks, and that is normal — a link opened from a chat, an app
/// or the address bar sends no `Referer` at all.
pub fn referer_host(headers: &HeaderMap) -> String {
    let raw = header(headers, "referer");

    Url::parse(raw)
        .ok()
        .and_then(|url| url.host_str().map(str::to_ascii_lowercase))
        .filter(|host| host.len() <= 253)
        .unwrap_or_default()
}

/// Classifies the user-agent into the four buckets the dashboard groups by.
///
/// Order is the whole algorithm here, because the strings nest: an iPhone claims
/// `like Mac OS X`, Android claims `Linux`, Chrome claims `Safari`, and Edge
/// claims both Chrome and Safari. Every rule below is written so the most specific
/// claim is tested first — reordering them silently reclassifies traffic instead
/// of failing.
pub fn agent(user_agent: &str) -> Agent {
    let ua = user_agent.to_ascii_lowercase();

    // First, because a crawler's device and browser are noise. A bot that also
    // looks like Chrome is a bot; classifying it as a desktop visitor is the error
    // that makes every other number wrong.
    if BOT_MARKERS.iter().any(|marker| ua.contains(marker)) {
        return Agent {
            device: "bot",
            os: OTHER,
            browser: OTHER,
            is_bot: 1,
        };
    }

    if ua.is_empty() {
        return Agent {
            device: OTHER,
            os: OTHER,
            browser: OTHER,
            is_bot: 0,
        };
    }

    Agent {
        device: device(&ua),
        os: os(&ua),
        browser: browser(&ua),
        is_bot: 0,
    }
}

/// Android's own rule: the tablet build omits `Mobile`, the phone build includes
/// it. That is the vendor's convention and the only reliable way to tell them
/// apart from the string.
fn device(ua: &str) -> &'static str {
    if ua.contains("ipad") || (ua.contains("android") && !ua.contains("mobile")) {
        return "tablet";
    }

    if ua.contains("mobile") || ua.contains("iphone") || ua.contains("ipod") {
        return "mobile";
    }

    "desktop"
}

/// iOS before macOS, and Android before Linux — both because the narrower system
/// advertises the wider one. An iPhone says `like Mac OS X`; Android is built on
/// Linux and says so.
fn os(ua: &str) -> &'static str {
    if ua.contains("windows") {
        "windows"
    } else if ua.contains("android") {
        "android"
    } else if ua.contains("iphone") || ua.contains("ipad") || ua.contains("ipod") {
        "ios"
    } else if ua.contains("mac os") {
        "macos"
    } else if ua.contains("cros") {
        "chromeos"
    } else if ua.contains("linux") {
        "linux"
    } else {
        OTHER
    }
}

/// Every rule before Safari exists because that token is in almost every string.
/// Chrome ships it, Edge ships Chrome's whole tail, and Opera ships Chrome's.
/// Testing Safari first would file most of the web under it.
fn browser(ua: &str) -> &'static str {
    if ua.contains("edg/") || ua.contains("edga/") || ua.contains("edgios/") {
        "edge"
    } else if ua.contains("opr/") || ua.contains("opera") {
        "opera"
    } else if ua.contains("samsungbrowser") {
        "samsung"
    } else if ua.contains("firefox") || ua.contains("fxios") {
        "firefox"
    } else if ua.contains("chrome") || ua.contains("crios") {
        "chrome"
    } else if ua.contains("safari") {
        "safari"
    } else {
        OTHER
    }
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue};

    use super::{Agent, agent, country, lang, referer_host};

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            if let Ok(value) = HeaderValue::from_str(value) {
                map.insert(*name, value);
            }
        }
        map
    }

    /// Real strings, because invented ones agree with whatever rule wrote them.
    /// Each row is a claim about traffic this shortener actually receives.
    #[test]
    fn real_user_agents_land_in_the_right_buckets() {
        let cases: [(&str, Agent); 8] = [
            (
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
                Agent {
                    device: "desktop",
                    os: "windows",
                    browser: "chrome",
                    is_bot: 0,
                },
            ),
            // Claims Chrome and Safari; only the `Edg/` token tells the truth.
            (
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36 Edg/120.0.0.0",
                Agent {
                    device: "desktop",
                    os: "windows",
                    browser: "edge",
                    is_bot: 0,
                },
            ),
            // "like Mac OS X" — iOS has to win over macOS.
            (
                "Mozilla/5.0 (iPhone; CPU iPhone OS 17_1 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.1 Mobile/15E148 Safari/604.1",
                Agent {
                    device: "mobile",
                    os: "ios",
                    browser: "safari",
                    is_bot: 0,
                },
            ),
            // Android says Linux; Android has to win.
            (
                "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Mobile Safari/537.36",
                Agent {
                    device: "mobile",
                    os: "android",
                    browser: "chrome",
                    is_bot: 0,
                },
            ),
            // Android without `Mobile` is the vendor's tablet convention.
            (
                "Mozilla/5.0 (Linux; Android 13; SM-X710) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
                Agent {
                    device: "tablet",
                    os: "android",
                    browser: "chrome",
                    is_bot: 0,
                },
            ),
            (
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10.15; rv:121.0) Gecko/20100101 Firefox/121.0",
                Agent {
                    device: "desktop",
                    os: "macos",
                    browser: "firefox",
                    is_bot: 0,
                },
            ),
            // An unfurler that never says "bot", and claims to be Chrome.
            (
                "WhatsApp/2.23.20.0 A",
                Agent {
                    device: "bot",
                    os: "other",
                    browser: "other",
                    is_bot: 1,
                },
            ),
            (
                "Mozilla/5.0 (compatible; Googlebot/2.1; +http://www.google.com/bot.html)",
                Agent {
                    device: "bot",
                    os: "other",
                    browser: "other",
                    is_bot: 1,
                },
            ),
        ];

        for (ua, expected) in cases {
            assert_eq!(agent(ua), expected, "misread: {ua}");
        }
    }

    /// The rule that protects every other number: a crawler dressed as a browser
    /// is still a crawler.
    #[test]
    fn a_bot_pretending_to_be_chrome_is_still_a_bot() {
        let ua = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36 SomeCrawler/1.0";

        let parsed = agent(ua);
        assert_eq!(parsed.is_bot, 1);
        assert_eq!(parsed.device, "bot");
    }

    #[test]
    fn an_absent_user_agent_is_not_a_bot_and_not_a_desktop() {
        assert_eq!(
            agent(""),
            Agent {
                device: "other",
                os: "other",
                browser: "other",
                is_bot: 0
            }
        );
    }

    #[test]
    fn country_comes_from_cloudflare_and_is_bounded() {
        assert_eq!(country(&headers(&[("cf-ipcountry", "br")])), "BR");
        // Cloudflare's own value for "could not tell" — kept, not blanked.
        assert_eq!(country(&headers(&[("cf-ipcountry", "XX")])), "XX");
        assert_eq!(country(&HeaderMap::new()), "");
        // Client-supplied on any path that skips Cloudflare, so junk is refused
        // rather than stored in a LowCardinality column.
        assert_eq!(country(&headers(&[("cf-ipcountry", "not-a-country")])), "");
    }

    #[test]
    fn language_keeps_the_primary_subtag_only() {
        assert_eq!(
            lang(&headers(&[("accept-language", "pt-BR,pt;q=0.9,en;q=0.8")])),
            "pt"
        );
        assert_eq!(lang(&headers(&[("accept-language", "en-GB")])), "en");
        assert_eq!(lang(&headers(&[("accept-language", "*")])), "");
        assert_eq!(lang(&HeaderMap::new()), "");
    }

    /// The host and nothing else. A referring path can carry a query string, a
    /// session token or a private document name.
    #[test]
    fn referer_keeps_the_host_and_drops_the_path() {
        assert_eq!(
            referer_host(&headers(&[(
                "referer",
                "https://News.YCombinator.com/item?id=1&token=secret"
            )])),
            "news.ycombinator.com"
        );
        assert_eq!(referer_host(&headers(&[("referer", "not a url")])), "");
        assert_eq!(referer_host(&HeaderMap::new()), "");
    }
}
