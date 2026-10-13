use percent_encoding::percent_decode_str;
use reqwest::{header, Client};
use url::Url;

use crate::{net, Error, Result};

#[derive(Debug, Clone, PartialEq)]
pub struct ProbeResult {
    pub final_url: Url,
    pub length: Option<u64>,
    pub accepts_ranges: bool,
    pub filename: String,
    /// ETag, else Last-Modified; used to detect that the remote file changed between sessions.
    pub validator: Option<String>,
}

/// `GET` + `Range: bytes=0-0` (works on servers that reject HEAD). The body is never read.
pub async fn run(client: &Client, url: &Url, headers: &[(String, String)]) -> Result<ProbeResult> {
    let resp = net::apply_headers(client.get(url.clone()), headers, url, url)
        .header(header::RANGE, "bytes=0-0")
        .send()
        .await?;
    let h = resp.headers();
    let get = |name: header::HeaderName| h.get(name).and_then(|v| v.to_str().ok()).map(str::to_owned);

    let (length, ranges) = match resp.status().as_u16() {
        206 => match get(header::CONTENT_RANGE)
            .and_then(|cr| cr.rsplit('/').next().and_then(|t| t.trim().parse::<u64>().ok()))
        {
            Some(n) => (Some(n), true),
            None => (None, false),
        },
        200 => (resp.content_length().filter(|&n| n > 0), false),
        code => return Err(Error::BadResponse(code)),
    };

    let final_url = resp.url().clone();
    let filename = match get(header::CONTENT_DISPOSITION).as_deref().and_then(parse_content_disposition) {
        Some(name) => sanitize(&name),
        None => {
            let last = final_url.path_segments().and_then(|mut s| s.next_back()).unwrap_or("");
            sanitize(if last.is_empty() { "download" } else { last })
        }
    };
    Ok(ProbeResult {
        validator: get(header::ETAG).or_else(|| get(header::LAST_MODIFIED)),
        final_url,
        length,
        accepts_ranges: ranges,
        filename,
    })
}

/// RFC 6266: prefers `filename*=UTF-8''...` over `filename="..."`.
pub fn parse_content_disposition(value: &str) -> Option<String> {
    let mut plain = None;
    for part in value.split(';').map(str::trim) {
        let lower = part.to_ascii_lowercase();
        if lower.starts_with("filename*=") {
            let v = &part["filename*=".len()..];
            if let Some((_, enc)) = v.split_once("''") {
                if let Ok(dec) = percent_decode_str(enc).decode_utf8() {
                    return Some(dec.into_owned());
                }
            }
        } else if lower.starts_with("filename=") {
            let mut v = &part["filename=".len()..];
            if v.len() >= 2 && v.starts_with('"') && v.ends_with('"') {
                v = &v[1..v.len() - 1];
            }
            plain = Some(v.to_owned());
        }
    }
    plain
}

/// Unicode general category Cf (format). Rust's std only knows Cc, so the Cf table is spelled out.
fn is_format(c: char) -> bool {
    matches!(c as u32,
        0xAD | 0x600..=0x605 | 0x61C | 0x6DD | 0x70F | 0x890..=0x891 | 0x8E2 | 0x180E
        | 0x200B..=0x200F | 0x202A..=0x202E | 0x2060..=0x2064 | 0x2066..=0x206F | 0xFEFF
        | 0xFFF9..=0xFFFB | 0x110BD | 0x110CD | 0x13430..=0x1343F | 0x1BCA0..=0x1BCA3
        | 0x1D173..=0x1D17A | 0xE0001 | 0xE0020..=0xE007F)
}

/// Removes control (Cc) and format (Cf) characters.
pub fn strip_invisible(s: &str) -> String {
    s.chars().filter(|&c| !c.is_control() && !is_format(c)).collect()
}

const RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9",
    "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];
const MAX_NAME_CHARS: usize = 200;

/// Makes a server-supplied name safe to create in the downloads folder.
pub fn sanitize(name: &str) -> String {
    let decoded = percent_decode_str(name).decode_utf8().map(|c| c.into_owned()).unwrap_or_else(|_| name.to_owned());
    // Control and invisible format characters (e.g. U+202E right-to-left override) can disguise the real extension.
    let replaced: String = strip_invisible(&decoded)
        .chars()
        .map(|c| if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') { '_' } else { c })
        .collect();
    // Windows silently drops trailing dots and spaces, which would change the name we locked.
    let mut cleaned = replaced.trim().trim_start_matches('.').trim_end_matches(['.', ' ']).to_owned();
    if cleaned.is_empty() {
        return "download".into();
    }
    if cleaned.chars().count() > MAX_NAME_CHARS {
        let (stem, ext) = split_ext(&cleaned);
        let ext = ext.filter(|e| e.chars().count() <= 16).map(|e| format!(".{e}")).unwrap_or_default();
        let stem: String = stem.chars().take(MAX_NAME_CHARS - ext.chars().count()).collect();
        cleaned = format!("{}{ext}", stem.trim_end_matches(['.', ' ']));
    }
    let device = cleaned.split('.').next().unwrap_or("").trim_end().to_ascii_uppercase();
    if RESERVED.contains(&device.as_str()) {
        cleaned.insert(0, '_');
    }
    cleaned
}

/// `("archive.tar", Some("gz"))` for "archive.tar.gz"; no extension for "README" or ".hidden".
pub fn split_ext(name: &str) -> (&str, Option<&str>) {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && !ext.is_empty() => (stem, Some(ext)),
        _ => (name, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_disposition() {
        assert_eq!(parse_content_disposition("attachment; filename=\"a b.zip\"").as_deref(), Some("a b.zip"));
        assert_eq!(
            parse_content_disposition("attachment; filename=\"x.txt\"; filename*=UTF-8''%E2%82%AC.txt").as_deref(),
            Some("€.txt")
        );
    }

    #[test]
    fn sanitize_blocks_traversal() {
        assert_eq!(sanitize("../../etc/passwd"), "_.._etc_passwd");
        assert_eq!(sanitize("..\\..\\Windows\\win.ini"), "_.._Windows_win.ini");
        assert_eq!(sanitize("..."), "download");
    }

    #[test]
    fn sanitize_strips_invisible_characters() {
        assert_eq!(sanitize("photo\u{202E}gpj.command"), "photogpj.command");
        assert_eq!(sanitize("a%0Ab.txt"), "ab.txt");
    }

    #[test]
    fn sanitize_handles_windows_rules() {
        assert_eq!(sanitize("a<b>c|d?.txt"), "a_b_c_d_.txt");
        assert_eq!(sanitize("con.txt"), "_con.txt");
        assert_eq!(sanitize("NUL"), "_NUL");
        assert_eq!(sanitize("console.log"), "console.log");
        assert_eq!(sanitize("report. . "), "report");
        let long = format!("{}.mp4", "x".repeat(400));
        let s = sanitize(&long);
        assert!(s.ends_with(".mp4") && s.chars().count() == MAX_NAME_CHARS);
    }
}
