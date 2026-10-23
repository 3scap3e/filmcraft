//! HLS media playlists of fragmented MP4 (`#EXT-X-MAP` init segment plus `.m4s` fragments), the
//! form Reactor hands clips over in. Parsing and assembly only: fetching is the caller's.

use crate::GenerateError;

/// The segments a media playlist names, in playback order, as absolute URLs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Playlist {
    pub init_url: String,
    pub segment_urls: Vec<String>,
}

/// Longest playlist body parsed (a clip is a few dozen lines).
const MAX_PLAYLIST_BYTES: usize = 1 << 20;
/// Most segments taken from one playlist.
const MAX_SEGMENTS: usize = 10_000;

/// Parse an `.m3u8` body. Relative URLs resolve against `playlist_url`.
pub fn parse(body: &str, playlist_url: &str) -> Result<Playlist, GenerateError> {
    if body.len() > MAX_PLAYLIST_BYTES {
        return Err(GenerateError::Protocol("the playlist is too large".into()));
    }
    let mut init_url = None;
    let mut segment_urls = Vec::new();
    for raw in body.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(attrs) = line.strip_prefix("#EXT-X-MAP:") {
            if let Some(uri) = attribute(attrs, "URI") {
                init_url = Some(resolve(playlist_url, uri));
            }
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        if segment_urls.len() >= MAX_SEGMENTS {
            return Err(GenerateError::Protocol("the playlist names too many segments".into()));
        }
        segment_urls.push(resolve(playlist_url, line));
    }
    if segment_urls.is_empty() {
        return Err(GenerateError::Protocol("the playlist names no segments".into()));
    }
    let init_url = init_url.ok_or_else(|| GenerateError::Protocol("the playlist has no #EXT-X-MAP init segment".into()))?;
    Ok(Playlist { init_url, segment_urls })
}

/// The value of `name="…"` in an attribute list.
fn attribute<'a>(attrs: &'a str, name: &str) -> Option<&'a str> {
    let mut rest = attrs;
    while !rest.is_empty() {
        let (key, after) = rest.split_once('=')?;
        let (value, tail) = if let Some(q) = after.strip_prefix('"') {
            let end = q.find('"')?;
            (&q[..end], q.get(end + 1..).unwrap_or("").trim_start_matches(','))
        } else {
            match after.split_once(',') {
                Some((v, t)) => (v, t),
                None => (after, ""),
            }
        };
        if key.trim() == name {
            return Some(value);
        }
        rest = tail;
    }
    None
}

/// `reference` resolved against `base` (absolute references pass through; path-only references
/// take the base's origin; relative ones replace the base's last path segment).
pub fn resolve(base: &str, reference: &str) -> String {
    if reference.contains("://") {
        return reference.to_string();
    }
    let (scheme_host, path) = match base.find("://").and_then(|i| base.get(i + 3..).and_then(|rest| rest.find('/')).map(|j| i + 3 + j)) {
        Some(split) => (base.get(..split).unwrap_or(base), base.get(split..).unwrap_or("/")),
        None => (base, "/"),
    };
    let path = path.split(['?', '#']).next().unwrap_or("/");
    if reference.starts_with('/') {
        return format!("{scheme_host}{reference}");
    }
    let dir = match path.rfind('/') {
        Some(i) => path.get(..=i).unwrap_or("/"),
        None => "/",
    };
    format!("{scheme_host}{dir}{reference}")
}

/// Fragmented MP4 is self-describing: the init segment (`ftyp` + `moov`) followed by each
/// fragment (`moof` + `mdat`) in order is a complete, playable file. `init` and every segment are
/// checked to start with a box header whose declared size fits the data.
pub fn assemble(init: &[u8], segments: &[Vec<u8>]) -> Result<Vec<u8>, GenerateError> {
    check_boxes(init, "init segment")?;
    let mut total = init.len();
    for (i, s) in segments.iter().enumerate() {
        check_boxes(s, &format!("segment {}", i + 1))?;
        total = total.checked_add(s.len()).ok_or_else(|| GenerateError::Protocol("the clip is too large".into()))?;
    }
    let mut out = Vec::new();
    out.try_reserve_exact(total).map_err(|e| GenerateError::Protocol(format!("unable to allocate the clip: {e}")))?;
    out.extend_from_slice(init);
    for s in segments {
        out.extend_from_slice(s);
    }
    Ok(out)
}

/// Every top-level box of `data` has a sane header and fits.
fn check_boxes(data: &[u8], what: &str) -> Result<(), GenerateError> {
    let mut pos = 0usize;
    let mut boxes = 0usize;
    while pos < data.len() {
        let header = data.get(pos..pos + 8).ok_or_else(|| GenerateError::Protocol(format!("{what}: truncated box header")))?;
        let size32 = u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as usize;
        let kind = &header[4..8];
        if !kind.iter().all(|b| b.is_ascii_alphanumeric() || *b == b' ' || *b == b'-' || *b == 0xa9) {
            return Err(GenerateError::Protocol(format!("{what}: not an MP4 box")));
        }
        let size = match size32 {
            0 => data.len().saturating_sub(pos),
            1 => {
                let large = data.get(pos + 8..pos + 16).ok_or_else(|| GenerateError::Protocol(format!("{what}: truncated large box")))?;
                usize::try_from(u64::from_be_bytes([large[0], large[1], large[2], large[3], large[4], large[5], large[6], large[7]]))
                    .map_err(|_| GenerateError::Protocol(format!("{what}: box too large")))?
            }
            n => n,
        };
        if size < 8 {
            return Err(GenerateError::Protocol(format!("{what}: bad box size")));
        }
        pos = pos.checked_add(size).filter(|end| *end <= data.len()).ok_or_else(|| GenerateError::Protocol(format!("{what}: box runs past the end")))?;
        boxes += 1;
    }
    if boxes == 0 {
        return Err(GenerateError::Protocol(format!("{what}: empty")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bx(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut v = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(kind);
        v.extend_from_slice(payload);
        v
    }

    #[test]
    fn playlists_resolve_relative_init_and_segment_urls() {
        let body = "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-TARGETDURATION:2\n#EXT-X-MAP:URI=\"init.mp4\",BYTERANGE=\"1@0\"\n#EXTINF:2.0,\nseg1.m4s\n#EXTINF:2.0,\n/abs/seg2.m4s\n#EXTINF:2.0,\nhttps://cdn.example/seg3.m4s?sig=1\n#EXT-X-ENDLIST\n";
        let p = parse(body, "https://rt.example/sessions/s1/clips/c1/index.m3u8?token=abc").unwrap();
        assert_eq!(p.init_url, "https://rt.example/sessions/s1/clips/c1/init.mp4");
        assert_eq!(
            p.segment_urls,
            vec!["https://rt.example/sessions/s1/clips/c1/seg1.m4s", "https://rt.example/abs/seg2.m4s", "https://cdn.example/seg3.m4s?sig=1"]
        );
    }

    #[test]
    fn playlists_without_segments_or_init_are_refused() {
        assert!(parse("#EXTM3U\n#EXT-X-MAP:URI=\"i.mp4\"\n", "https://a/b.m3u8").is_err());
        assert!(parse("#EXTM3U\nseg.m4s\n", "https://a/b.m3u8").is_err());
        assert!(parse(&"x".repeat(MAX_PLAYLIST_BYTES + 1), "https://a/b.m3u8").is_err());
    }

    #[test]
    fn assembly_concatenates_checked_boxes() {
        let init = [bx(b"ftyp", b"iso5"), bx(b"moov", &[0; 4])].concat();
        let seg = [bx(b"moof", &[1; 2]), bx(b"mdat", &[2; 9])].concat();
        let out = assemble(&init, &[seg.clone(), seg.clone()]).unwrap();
        assert_eq!(out.len(), init.len() + 2 * seg.len());
        assert_eq!(&out[..init.len()], &init[..]);
        assert!(assemble(b"notmp4!!", &[]).is_err());
        let mut truncated = seg.clone();
        truncated.truncate(seg.len() - 1);
        assert!(assemble(&init, &[truncated]).is_err());
        assert!(assemble(&[], &[seg]).is_err());
        let mut bad = bx(b"mdat", &[0; 4]);
        bad[3] = 4;
        assert!(assemble(&bad, &[]).is_err());
    }
}
