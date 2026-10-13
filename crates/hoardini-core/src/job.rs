use serde::{Deserialize, Serialize};
use url::Url;

use crate::probe;

/// A download request as sent by the browser extension (via the native host) or typed into the app.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobRequest {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub referrer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cookies: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_agent: Option<String>,
    /// "file" | "hls"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    /// HLS only: the variant playlist picked in the popup, with `url` being its master.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
}

impl JobRequest {
    pub fn new(url: impl Into<String>) -> Self {
        Self { url: url.into(), ..Default::default() }
    }

    /// Only plain http(s) URLs with a host are accepted.
    pub fn valid_url(&self) -> Option<Url> {
        http_url(&self.url)
    }

    pub fn valid_variant(&self) -> Option<Url> {
        self.variant.as_deref().and_then(http_url)
    }

    pub fn is_hls(&self) -> bool {
        self.kind.as_deref() == Some("hls") || self.valid_url().is_some_and(|u| url_extension(&u) == "m3u8")
    }

    /// Header values containing line breaks are dropped (header injection).
    pub fn headers(&self) -> Vec<(String, String)> {
        [("Referer", &self.referrer), ("Cookie", &self.cookies), ("User-Agent", &self.user_agent)]
            .into_iter()
            .filter_map(|(name, value)| {
                let v = value.as_deref()?;
                (!v.is_empty() && !v.contains(['\r', '\n'])).then(|| (name.to_owned(), v.to_owned()))
            })
            .collect()
    }

    pub fn clean_filename(&self) -> Option<String> {
        let f = self.filename.as_deref()?;
        let s: String = probe::strip_invisible(f).chars().take(150).collect();
        let s = s.trim();
        (!s.is_empty()).then(|| s.to_owned())
    }
}

pub fn http_url(s: &str) -> Option<Url> {
    let u = Url::parse(s).ok()?;
    let ok = matches!(u.scheme(), "http" | "https") && u.host_str().is_some_and(|h| !h.is_empty());
    ok.then_some(u)
}

/// Lower-cased extension of the URL's last path component ("" when there is none).
pub fn url_extension(u: &Url) -> String {
    let last = u.path_segments().and_then(|mut s| s.next_back()).unwrap_or("");
    probe::split_ext(last).1.unwrap_or("").to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_extension_message() {
        let j: JobRequest = serde_json::from_str(
            r#"{"url":"https://e.x/v/master.m3u8","referrer":"","kind":"file","filename":"","variant":"","cookies":"a=b","userAgent":"UA"}"#,
        )
        .unwrap();
        assert!(j.is_hls());
        assert_eq!(j.headers(), vec![("Cookie".into(), "a=b".into()), ("User-Agent".into(), "UA".into())]);
        assert_eq!(j.clean_filename(), None);
        assert_eq!(j.valid_variant(), None);
    }

    #[test]
    fn rejects_non_http_and_header_injection() {
        assert!(JobRequest::new("file:///C:/x").valid_url().is_none());
        assert!(JobRequest::new("javascript:alert(1)").valid_url().is_none());
        let j = JobRequest { referrer: Some("https://a\r\nX-Evil: 1".into()), ..JobRequest::new("https://a/b") };
        assert!(j.headers().is_empty());
    }
}
